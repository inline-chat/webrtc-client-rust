// let's provide 3 detail levels, balance, text, and motion to adjust QP
pub struct CapturerConfig {
  fps: Option<i32>,
  priority: Priority,
  resolution: Resolution,
  // captures_audio: bool,
}

pub enum Priority {
  Balanced,
  Motion,
  Quality,
}

pub enum Resolution {
  Best,
  Nominal,
  Low,
}

impl CapturerConfig {
  pub fn default() -> Self {
    Self {
      fps: None,
      priority: Priority::Balanced,
      resolution: Resolution::Nominal,
      // captures_audio: false,
    }
  }

  pub fn resolution_as_scale_factor(&self) -> f32 {
    match &self.resolution {
      Resolution::Best => 2.0,
      Resolution::Nominal => 1.0,
      Resolution::Low => 0.5,
    }
  }

  pub fn resolution(&self) -> &Resolution {
    &self.resolution
  }

  pub fn set_resolution(&mut self, resolution: Resolution) {
    self.resolution = resolution;
  }

  pub fn priority(&self) -> &Priority {
    &self.priority
  }

  pub fn set_priority(&mut self, priority: Priority) {
    self.priority = priority;
  }

  pub fn effective_fps(&self) -> i32 {
    if let Some(fps) = self.fps {
      return fps;
    }

    match (&self.resolution, &self.priority) {
      (&Resolution::Best, &Priority::Motion) => 60,
      (&Resolution::Nominal, &Priority::Motion) => 30,
      (&Resolution::Low, &Priority::Motion) => 24,

      (&Resolution::Best, &Priority::Quality) => 30,
      (&Resolution::Nominal, &Priority::Quality) => 20,
      (&Resolution::Low, &Priority::Quality) => 15,

      (&Resolution::Best, &Priority::Balanced) => 60,
      (&Resolution::Nominal, &Priority::Balanced) => 24,
      (&Resolution::Low, &Priority::Balanced) => 20,
    }
  }

  // QP is from 1-51 which lower indicates better quality at the expense of bitrate
  // and higher indicates lower quality.
  pub fn suggested_max_qp(&self) -> i32 {
    match (&self.resolution, &self.priority) {
      (&Resolution::Best, &Priority::Motion) => 40,
      (&Resolution::Nominal, &Priority::Motion) => 40,
      (&Resolution::Low, &Priority::Motion) => 51,

      (&Resolution::Best, &Priority::Quality) => 20,
      (&Resolution::Nominal, &Priority::Quality) => 45,
      (&Resolution::Low, &Priority::Quality) => 51,

      (&Resolution::Best, &Priority::Balanced) => 30,
      (&Resolution::Nominal, &Priority::Balanced) => 40,
      (&Resolution::Low, &Priority::Balanced) => 40,
    }
  }

  /// Provide your encoder width and height to get desired initial bitrate
  /// This will later be overriden with estimates from network.
  pub fn initial_bitrate(&self, width: u32, height: u32) -> i32 {
    // above 30 is high fps (i.e. 60)
    let is_high_fps = self.effective_fps() > 30;

    // 1440x900 = 2mbps
    let base = ((width as f64 * height as f64) * 1.6) as i32;
    if is_high_fps {
      (base as f32 * 1.5) as i32
    } else {
      base
    }
  }

  pub fn fps(&self) -> Option<i32> {
    self.fps
  }

  pub fn set_fps(&mut self, fps: Option<i32>) {
    self.fps = fps;
  }
}
