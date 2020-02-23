extern crate notify_rust;
extern crate nix;
use structopt::StructOpt;
use std::iter::FromIterator;

extern crate pulsectl;
extern crate libpulse_binding as pulse;

use pulsectl::controllers::DeviceControl;
use pulsectl::controllers::SinkController;
use pulsectl::controllers::AppControl;

trait VolumeControlTarget {
    fn is_running(&self) -> bool;
    fn name(&self) -> String;
    fn description(&self) -> String;
    fn is_default(&self, controller: &mut SinkController) -> bool;
    fn is_muted(&self) -> bool;
    fn mute(&self, mute: bool, controller: &mut SinkController);
    fn volume(&self) -> pulse::volume::ChannelVolumes;
    fn set_volume(&self, new_vol: pulse::volume::ChannelVolumes, controller: &mut SinkController);
}

impl VolumeControlTarget for pulsectl::controllers::types::DeviceInfo {
    fn is_running(&self) -> bool {
        self.state == pulsectl::controllers::types::DevState::Running
    }
    fn name(&self) -> std::string::String {
        self.name.as_ref().unwrap().to_string()
    }
    fn is_default(&self, controller: &mut SinkController) -> bool {
        self.name() == controller.get_server_info().unwrap().default_sink_name.unwrap()
    }
    fn is_muted(&self) -> bool { self.mute }
    fn mute(&self, mute: bool, controller: &mut SinkController) {
        let op = controller.handler.introspect.set_sink_mute_by_name(self.name.as_ref().unwrap(), mute, Option::Some(Box::new(|_| {})));
        controller.handler.wait_for_operation(op).unwrap();
    }
    fn description(&self) -> std::string::String {
        self.description.as_ref().unwrap().to_string()
    }
    fn volume(&self) -> pulse::volume::ChannelVolumes { self.volume }
    fn set_volume(&self, new_vol: pulse::volume::ChannelVolumes, controller: &mut SinkController) {
        controller.set_device_volume_by_name(&self.name(), &new_vol);
    }
}

impl VolumeControlTarget for pulsectl::controllers::types::ApplicationInfo {
    fn is_running(&self) -> bool { !self.corked }
    fn name(&self) -> std::string::String {
        self.name.as_ref().unwrap().to_string()
    }
    fn is_default(&self, _: &mut SinkController) -> bool { false }
    fn is_muted(&self) -> bool { self.mute }
    fn mute(&self, mute: bool, controller: &mut SinkController) {
        controller.set_app_mute(self.index, mute).unwrap();
    }
    fn description(&self) -> std::string::String {
        match self.proplist.get_str("application.name") {
            Some(desc) => desc,
            None => "Unspecified".to_string(),
        }
    }
    fn volume(&self) -> pulse::volume::ChannelVolumes { self.volume }
    fn set_volume(&self, new_vol: pulse::volume::ChannelVolumes, controller: &mut SinkController) {
        let op = controller.handler.introspect.set_sink_input_volume(self.index, &new_vol, None);
        controller.handler.wait_for_operation(op).unwrap();
    }
}

#[derive(Debug, StructOpt)]
#[structopt(name = env!("CARGO_PKG_NAME"), about = "Very simple program to control volume of currently playing device", author = "Mikhail Khvoinitsky")]
struct Opt {
    /// Verbose mode
    #[structopt(short, long)]
    verbose: bool,

    /// Control application, not device
    #[structopt(short, long)]
    application: bool,

    /// Increase volume of PERCENT
    #[structopt(long, value_name = "PERCENT", display_order(1))]
    increase: Option<f64>,

    /// Decrease volume of PERCENT
    #[structopt(long, value_name = "PERCENT", display_order(1), conflicts_with = "increase")]
    decrease: Option<f64>,

    /// Toggle mute
    #[structopt(short, long, display_order(1), conflicts_with = "increase")]
    mute_toggle: bool,

    /// Notification duration
    #[structopt(long, default_value = "1", display_order(10), value_name = "SECONDS")]
    duration: f64,

    /// Notification icon
    #[structopt(long, default_value = "audio-volume-muted", display_order(11), value_name = "ICON")]
    icon_muted: std::string::String,

    /// Notification icon
    #[structopt(long, default_value = "audio-volume-high", display_order(11), value_name = "ICON")]
    icon: std::string::String,

    /// Notification title
    #[structopt(long, default_value = "Volume", display_order(12), value_name = "ICON")]
    title: std::string::String,

    /// Steps for smooth transition (1 to disable)
    #[structopt(long, default_value = "1", display_order(20), value_name = "NUMBER")]
    steps: u64,

    /// Step interval
    #[structopt(long, default_value = "0", display_order(21), value_name = "MILLISECONDS")]
    step_interval: u64,
}

fn adjust_volume(targets: Vec<impl VolumeControlTarget>, opt: Opt, tmpfile_base: &str, mut controller: SinkController) {
    let prev_device_filename = format!("{}_{}", tmpfile_base, "previously_controlled_device_name");
    let (devices_to_control, write_prev_device) = {
        let running_devices: Vec<_> = targets.iter().filter(|target| target.is_running()).collect();
        if running_devices.len() > 0 {
            println!("running_devices");
            (running_devices, true)
        } else {
            let prev_devices = match std::fs::read_to_string(&prev_device_filename) {
                Ok(prev_device_name) => targets.iter().filter(|target| prev_device_name == target.name()).collect(),
                Err(_) => std::vec::Vec::<_>::new(),
            };
            if prev_devices.len() > 0 {
                println!("prev_devices");
                (prev_devices, false)
            } else {
                println!("fallback devices");
                // Warning: here get_server_info() is called for every device which is not very optimal TODO: fix it somehow
                (targets.iter().filter(|target| target.is_default(&mut controller)).collect(), false)
            }
        }
    };

    for device in devices_to_control {
        let prev_notification_id_filename = format!("{}_{}_{}", tmpfile_base, device.name(), "notification-id");
        let mut prev_id: u32 = match std::fs::read_to_string(&prev_notification_id_filename) {
            Ok(prev_notification_id_str) => match prev_notification_id_str.trim().parse::<u32>() {
                Ok(prev_notification_id) => prev_notification_id,
                Err(_) => 0,
            },
            Err(_) => 0,
        };

        prev_id = if opt.mute_toggle {
            let is_now_muted = !device.is_muted();
            device.mute(is_now_muted, &mut controller);

            show_notification(
                prev_id,
                if is_now_muted { pulse::volume::VOLUME_MUTED.0 } else { device.volume().avg().0 },
                &device.description(),
                if is_now_muted { opt.icon_muted.as_str() } else { opt.icon.as_str() },
                opt.duration,
            )
        } else {
            let mut muted = device.is_muted();
            println!("is muted: {}", muted);

            let old_volume = if muted && opt.increase.is_some() { pulse::volume::VOLUME_MUTED.0 } else { device.volume().avg().0 };
            let uimax = pulse::volume::Volume::ui_max().0;

            let factor: f64 = if opt.increase.is_some() {
                opt.increase.unwrap()
            } else {
                0f64 - opt.decrease.unwrap()
            } / 100.0;

            // TODO: less type conversions
            let new_volume_raw = (old_volume as f64 + ((pulse::volume::VOLUME_NORM.0 - pulse::volume::VOLUME_MUTED.0) as f64 * factor)).round() as i64;
            let new_volume = if opt.increase.is_some() {
                if old_volume >= uimax {
                    old_volume as i64  // if the volume is more than ui_max, the UI should not limit it and push the limited value back to the server.
                } else if old_volume < pulse::volume::VOLUME_NORM.0 && (pulse::volume::VOLUME_NORM.0 as i64) < new_volume_raw {
                    pulse::volume::VOLUME_NORM.0 as i64 // snap to 100% value
                } else {
                    i64::min(new_volume_raw, uimax as i64)
                }
            } else if opt.decrease.is_some() {
                if old_volume > pulse::volume::VOLUME_NORM.0 && (pulse::volume::VOLUME_NORM.0 as i64) > new_volume_raw {
                    pulse::volume::VOLUME_NORM.0 as i64 // snap to 100% value
                } else {
                    i64::max(new_volume_raw, pulse::volume::VOLUME_MUTED.0 as i64)
                }
            } else {
                panic!("something went wrong");
            } as u32;

            #[allow(unused_comparisons)]
            assert!(new_volume >= 0, "new_volume < 0");
            assert!(new_volume as u32 <= pulse::volume::VOLUME_MAX.0, "new_volume > VOLUME_MAX");

            let mut value_steps = std::vec::Vec::from_iter( (1..opt.steps + 1).map(|step_number| {
                if step_number == opt.steps {
                    new_volume
                } else {
                    (old_volume as f64 + (((new_volume as f64 - old_volume as f64) / opt.steps as f64) * step_number as f64)).round() as u32
                }
            }));
            value_steps.dedup();
            let value_steps_len = value_steps.len();

            for (new_volume, sleep_at_the_end) in value_steps.into_iter().enumerate().map(|(i, step_value)| {
                (step_value, i + 1 != value_steps_len)
            }) {
                let mut new_vol_struct = device.volume().clone();
                //new_vol_struct.set(new_vol_struct.len() as u32, pulse::volume::Volume(new_volume));
                for vol in new_vol_struct.get_mut() {
                    vol.0 = new_volume;
                }

                println!("old: {}, new: {}, uimax: {}", device.volume().avg().0, new_vol_struct.avg().0, uimax);
                device.set_volume(new_vol_struct, &mut controller);
                if muted && opt.increase.is_some() {
                    device.mute(false, &mut controller);
                    muted = false;
                }

                prev_id = show_notification(
                    prev_id,
                    new_volume,
                    &device.description(),
                    opt.icon.as_str(),
                    opt.duration,
                );

                if sleep_at_the_end {
                    std::thread::sleep(std::time::Duration::from_millis(opt.step_interval));
                }
            }

            if write_prev_device {
                std::fs::write(&prev_device_filename, device.name()).unwrap();
            };
            prev_id
        };
        std::fs::write(&prev_notification_id_filename, prev_id.to_string()).unwrap();
    }
}

fn main() {
    let opt = Opt::from_args();

    let mut tmp = std::env::temp_dir();
    tmp.push(env!("CARGO_PKG_NAME"));
    let tmpfile_base = tmp.to_str().unwrap();

    let lockfile_path = format!("{}.{}", tmpfile_base, "lock");
    let lockfile = std::fs::File::create(&lockfile_path).unwrap();
    nix::fcntl::flock(std::os::unix::io::AsRawFd::as_raw_fd(&lockfile), nix::fcntl::FlockArg::LockExclusiveNonblock).expect("Already running");

    let mut controller = SinkController::create();

    if opt.application {
        adjust_volume(controller.list_applications().expect("Could not get list of playback applications"), opt, tmpfile_base, controller);
    } else {
        adjust_volume(controller.list_devices().expect("Could not get list of playback devices"), opt, tmpfile_base, controller);
    }

    std::fs::remove_file(&lockfile_path).unwrap();
}

fn show_notification(prev_id: u32, new_volume: u32, dev_description: &String, icon: &str, duration: f64) -> u32 {
    let percentage = (100.0 * new_volume as f64 / pulse::volume::VOLUME_NORM.0 as f64).round() as i32;
    let progressbar_percentage = (100.0 * new_volume as f64 / pulse::volume::Volume::ui_max().0 as f64).round() as i32;

    println!("new: {}%, progress new: {}%", percentage, progressbar_percentage);

    let notification = notify_rust::Notification::new()
        .summary("Volume")
        .body(format!("{}% | {}", percentage, dev_description).as_str())
        .icon(icon)
        .hint(notify_rust::NotificationHint::CustomInt("value".to_string(), progressbar_percentage))
        .timeout(notify_rust::Timeout::Milliseconds((duration * 1000f64) as u32))
        .id(prev_id)
        .show().unwrap();
    return notification.id();
}
