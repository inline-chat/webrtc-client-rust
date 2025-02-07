use cap::{
  cf,
  cidre::{
    arc,
    core_audio::{
      AudioObjId, AudioObjPropAddr, AudioObjPropElement, AudioObjPropScope, AudioObjPropSelector,
    },
  },
};

#[tauri::command]
pub fn get_default_speaker() -> Result<String, ()> {
  let def_addr = AudioObjPropAddr {
    selector: AudioObjPropSelector::HARDWARE_DEFAULT_OUTPUT_DEVICE,
    scope: AudioObjPropScope::GLOBAL,
    element: AudioObjPropElement::MAIN,
  };

  let device: AudioObjId = AudioObjId::SYS_OBJECT.prop(&def_addr).unwrap();
  let name_addr = AudioObjPropAddr {
    selector: AudioObjPropSelector::NAME,
    scope: AudioObjPropScope::GLOBAL,
    element: AudioObjPropElement::MAIN,
  };

  let name: arc::R<cf::String> = device.cf_prop(&name_addr).unwrap();
  let device_name = name.to_string();

  Ok(device_name)
}

// #[cfg(test)]
// mod test {
//   use crate::macos;

//   #[test]
//   fn key_pressed() {
//     let hi = macos::get_default_speaker();
//   }
// }
