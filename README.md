# An implementation of WebRTC client stack built in Rust

> **Warning**: This will not run or compile. This is stripped away from the noor.to application, and is only for educational purposes.

## What is this?

When we started building [Noor](https://noor.to) in [Tauri](https://tauri.app/), we didn't want to use the WebKit WebRTC implementation. We wanted more control over the voice and screen sharing to achieve lower latency, higher quality, better performance, and a less-resource-intensive call experience. So we built this from scratch and learnt Rust and a whole lot about WebRTC and media processing while doing it.

It would've been super helpful if I had an easy-to-read open source implementation for a client stack to learn from. Since most of the WebRTC projects are for server side implementations, I'm sharing this to help other developers building cool client WebRTC applications without relying on libWebRTC if they don't need the whole thing.

## What exactly is implemented here?

This implements voice chat (cross-platform, built primarly for macOS but tested on Linux) and screen sharing (macOS-only). Specifically, it supports:

- WebRTC peer to peer connection
- WebRTC signaling and reconnection
- TURN / STUN / ICE
- Audio capture
- Audio processing
- Audio echo and noise cancellation
- Audio encoding and decoding
- Audio resampling
- Audio jitter buffer
- Audio playback
- Screen capture
- Hardware accelerated encoding and decoding
- GPU rendering on a CALayer (macOS)

I didn't get to finish the screen sharing part the way I wanted to, but it can be used as a starting point to get a basic idea of how an implementation would look like.

## Code guide

The file names are self-explanatory. However here's a few starting points:

- [engine.rs](./rtc/engine.rs) - The main entry point for the RTC engine that orchestrates the call across multiple peers.
- [jitter.rs](./rtc/jitter.rs) - The audio jitter buffer.
- [processor.rs](./rtc/processor.rs) - The audio processing pipeline.
- [audio_decoder.rs](./rtc/audio_decoder.rs) - The audio decoding.
- [audio_input.rs](./rtc/audio_input.rs) - The audio capture and processing.
- [resampler.rs](./rtc/resampler.rs) - The audio resampling.
- [player.rs](./rtc/player.rs) - The audio playback.
- [capturer.rs](./rtc/capturer.rs) - The screen capture and processing (was not finished)
- [gatherer.rs](./rtc/ice/gatherer.rs) - The ICE gatherer.

## Thanks to...

We used amazing open-source libraries to build this:

- [str0m](https://github.com/algesten/str0m)
- [cidre](https://github.com/yury/cidre/)
- [webrtc-audio-processing](https://crates.io/crates/webrtc-audio-processing)
- [webrtc-rs](https://github.com/webrtc-rs/webrtc)
- [tokio](https://crates.io/crates/tokio)
- [cpal](https://crates.io/crates/cpal)
- [ringbuf](https://crates.io/crates/ringbuf)
- [stun-client](https://github.com/yoshd/stun-client)
- [audio_thread_priority](https://crates.io/crates/audio_thread_priority)
- [opus](https://crates.io/crates/opus)
- [enigo](https://crates.io/crates/enigo)
- [icrate](https://crates.io/crates/icrate)
- [objc2](https://crates.io/crates/objc2)
- [core-graphics](https://crates.io/crates/core-graphics)
- [objc-foundation](https://crates.io/crates/objc-foundation)
- [flume](https://crates.io/crates/flume)
- [bytes](https://crates.io/crates/bytes)

And many other crates for building the application itself, mainly, [Tauri](https://tauri.app/).

While developing this, we contributed back to some of the crates mentioned above. One of which that I really enjoyed was adding support for keyboard noise cancellation through the libWebRTC's AEC to the [webrtc-audio-processing](https://crates.io/crates/webrtc-audio-processing) crate after a lot of reading through the WebRTC codebase for the first time I was looking at it.

# LICENSE

This code is licensed under the MIT license. See the LICENSE file for more details.
