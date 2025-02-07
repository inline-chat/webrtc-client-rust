extern crate anyhow;
extern crate cpal;
use cpal::traits::{DeviceTrait, HostTrait};
use serde::{Deserialize, Serialize};

fn enumerate() -> Result<Vec<AudioDevice>, anyhow::Error> {
  // let available_hosts = cpal::available_hosts();
  let mut audio_devices = Vec::new();

  #[cfg(any(
    not(any(target_os = "linux", target_os = "dragonfly", target_os = "freebsd")),
    not(feature = "jack")
  ))]
  let hosts = cpal::available_hosts();
  if hosts.is_empty() {
    return Ok(audio_devices);
  }
  let host = cpal::default_host();

  // for host_id in available_hosts {
  // println!("{}", host_id.name());
  // let host = cpal::host_from_id(host_id)?;

  let default_in = host
    .default_input_device()
    .map(|e| e.name().unwrap_or("Audio Device".to_string()));

  let default_out = host
    .default_output_device()
    .map(|e| e.name().unwrap_or("Audio Device".to_string()));

  let default_in_name: String = match default_in.as_deref() {
    Some(name) => name.to_owned(),
    None => "".to_string(),
  };

  let default_out_name: String = match default_out.as_deref() {
    Some(name) => name.to_owned(),
    None => "".to_string(),
  };

  if let Ok(devices) = host.devices() {
    for (_device_index, device) in devices.enumerate() {
      let is_default_device: bool;

      if default_in_name == device.name().unwrap_or("Audio Device".to_string()) {
        is_default_device = true;
      } else if default_out_name == device.name().unwrap_or("Audio Device".to_string()) {
        is_default_device = true;
      } else {
        is_default_device = false;
      }

      audio_devices.push(AudioDevice {
        kind: get_device_type(&device),
        name: device.name().unwrap_or("Audio Device".to_string()),
        default: is_default_device,
      });
    }
  }

  Ok(audio_devices)
}

fn get_device_type(device: &cpal::Device) -> AudioDeviceType {
  let input_configs = match device.supported_input_configs() {
    Ok(f) => f.collect(),
    Err(e) => {
      println!("    Error getting supported input configs: {:?}", e);
      Vec::new()
    }
  };
  if !input_configs.is_empty() {
    AudioDeviceType::Input
  } else {
    AudioDeviceType::Output
  }
}

#[derive(Serialize, Deserialize)]
enum AudioDeviceType {
  Output,
  Input,
}

#[derive(Serialize, Deserialize)]
pub struct AudioDevice {
  kind: AudioDeviceType,
  name: String,
  default: bool,
}

#[tauri::command]
pub async fn devices_list() -> Result<Vec<AudioDevice>, ()> {
  if let Ok(result) = enumerate() {
    Ok(result)
  } else {
    println!("failed to get device list");
    Ok(vec![])
  }
}

use async_process::Command;

#[tauri::command]
pub async fn get_speaker_volume() -> Result<String, ()> {
  let output = Command::new("osascript")
    .arg("-e")
    .arg("set outputVolume to output volume of (get volume settings)")
    .output()
    .await
    .expect("failed to run osascript");

  let volume = String::from_utf8(output.stdout).unwrap();

  Ok(volume.to_string())
}

#[tauri::command]
pub async fn get_is_speaker_muted() -> Result<String, ()> {
  let get_sound_code = r#"
  -- Check if Sound is Muted and Output 0 or 1
  tell application "System Events"
    set isMuted to output muted of (get volume settings)
    if isMuted then
      set mutedStatus to 1 -- 1 indicates muted
    else
      set mutedStatus to 0 -- 0 indicates not muted
    end if
  end tell

  return mutedStatus
  "#;

  let output = Command::new("osascript")
    .arg("-e")
    .arg(get_sound_code)
    .output()
    .await
    .expect("failed to run osascript");

  let muted = String::from_utf8(output.stdout).unwrap();

  Ok(muted.to_string())
}
