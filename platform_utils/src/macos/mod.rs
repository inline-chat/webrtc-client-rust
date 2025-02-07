pub mod idle_time;
pub mod version;

use core_graphics::event_source::CGEventSourceStateID;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
  fn CGEventSourceKeyState(source: CGEventSourceStateID, allowedKeyCode: CGKeyCode) -> bool;
}

type CGKeyCode = u16;

pub fn key_pressed() -> bool {
  let mut pressed = false;
  for key_code in 0..=0x5D {
    let key_state =
      unsafe { CGEventSourceKeyState(CGEventSourceStateID::HIDSystemState, key_code) };

    pressed |= key_state;

    if pressed {
      break;
    }
  }

  return pressed;
}

#[cfg(test)]
mod test {
  use crate::macos;

  #[test]
  fn key_pressed() {
    let _ = macos::key_pressed();
  }
}
