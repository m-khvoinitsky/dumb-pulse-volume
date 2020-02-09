extern crate notify_rust;
extern crate nix;
use structopt::StructOpt;
use std::iter::FromIterator;

extern crate pulsectl;
extern crate libpulse_binding as pulse;

use pulsectl::controllers::DeviceControl;
use pulsectl::controllers::SinkController;

#[derive(Debug, StructOpt)]
#[structopt(name = env!("CARGO_PKG_NAME"), about = "Very simple program to control volume of currently playing device", author = "Mikhail Khvoinitsky")]
struct Opt {
    /// Verbose mode
    #[structopt(short, long)]
    verbose: bool,

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

fn main() {
    let opt = Opt::from_args();

    let mut tmp = std::env::temp_dir();
    tmp.push(env!("CARGO_PKG_NAME"));
    let tmpfile_base = tmp.to_str().unwrap();

    let lockfile_path = format!("{}.{}", tmpfile_base, "lock");
    let lockfile = std::fs::File::create(&lockfile_path).unwrap();
    nix::fcntl::flock(std::os::unix::io::AsRawFd::as_raw_fd(&lockfile), nix::fcntl::FlockArg::LockExclusiveNonblock).expect("Already running");

    let mut handler = SinkController::create();
    let devices = handler
        .list_devices()
        .expect("Could not get list of playback devices");

    let prev_device_filename = format!("{}_{}", tmpfile_base, "previously_controlled_device_name");
    let (devices_to_control, write_prev_device) = {
        let running_devices: Vec<_> = devices.iter().filter(|dev| dev.state == pulsectl::controllers::types::DevState::Running).collect();
        if running_devices.len() > 0 {
            println!("running_devices");
            (running_devices, true)
        } else {
            let prev_devices = match std::fs::read_to_string(&prev_device_filename) {
                Ok(prev_device_name) => devices.iter().filter(|dev| prev_device_name == dev.name.as_ref().unwrap().as_str()).collect(),
                Err(_) => std::vec::Vec::<&pulsectl::controllers::types::DeviceInfo>::new(),
            };
            if prev_devices.len() > 0 {
                println!("prev_devices");
                (prev_devices, false)
            } else {
                println!("fallback devices");
                let default_sink_name = handler.get_server_info().unwrap().default_sink_name.unwrap();
                println!("default: {}", default_sink_name);
                (devices.iter().filter(|dev| default_sink_name == dev.name.as_ref().unwrap().as_str()).collect(), false)
            }
        }
    };

    for device in devices_to_control {
        let prev_notification_id_filename = format!("{}_{}_{}", tmpfile_base, device.name.as_ref().unwrap(), "notification-id");
        let mut prev_id: u32 = match std::fs::read_to_string(&prev_notification_id_filename) {
            Ok(prev_notification_id_str) => match prev_notification_id_str.trim().parse::<u32>() {
                Ok(prev_notification_id) => prev_notification_id,
                Err(_) => 0,
            },
            Err(_) => 0,
        };

        prev_id = if opt.mute_toggle {
            let op = handler.handler.introspect.set_sink_mute_by_name(device.name.as_ref().unwrap(), !device.mute, Option::Some(Box::new(|_| {})));
            handler.handler.wait_for_operation(op).unwrap();

            show_notification(
                prev_id,
                if !device.mute { pulse::volume::VOLUME_MUTED.0 } else { device.volume.avg().0 },
                device.description.as_ref().unwrap(),
                if !device.mute { opt.icon_muted.as_str() } else { opt.icon.as_str() },
                opt.duration,
            )
        } else {
            println!("is muted: {}", device.mute);
            let mut muted = device.mute;

            let old_volume = if muted && opt.increase.is_some() { pulse::volume::VOLUME_MUTED.0 } else { device.volume.avg().0 };
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
                let mut new_vol_struct = device.volume.clone();
                //new_vol_struct.set(new_vol_struct.len() as u32, pulse::volume::Volume(new_volume));
                for vol in new_vol_struct.get_mut() {
                    vol.0 = new_volume;
                }

                println!("old: {}, new: {}, uimax: {}", device.volume.avg().0, new_vol_struct.avg().0, uimax);
                handler.set_device_volume_by_name(device.name.as_ref().unwrap(), &new_vol_struct);
                if muted && opt.increase.is_some() {
                    let op = handler.handler.introspect.set_sink_mute_by_name(device.name.as_ref().unwrap(), false, Option::Some(Box::new(|_| {})));
                    handler.handler.wait_for_operation(op).unwrap();
                    muted = false;
                }

                prev_id = show_notification(
                    prev_id,
                    new_volume,
                    device.description.as_ref().unwrap(),
                    opt.icon.as_str(),
                    opt.duration,
                );

                if sleep_at_the_end {
                    std::thread::sleep(std::time::Duration::from_millis(opt.step_interval));
                }
            }

            if write_prev_device {
                std::fs::write(&prev_device_filename, device.name.as_ref().unwrap()).unwrap();
            };
            prev_id
        };
        std::fs::write(&prev_notification_id_filename, prev_id.to_string()).unwrap();
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
