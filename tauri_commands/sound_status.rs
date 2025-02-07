extern crate anyhow;
extern crate cpal;



use async_process::Command;

#[tauri::command]
pub async fn get_is_music_playing() -> Result<String, ()> {
  let playing_music_state = r#"
on is_running(appName)
  tell application "System Events" to (name of processes) contains appName
end is_running


set QTRunning to is_running("QuickTime Player")
set MusicRunning to is_running("Music")
set SpotifyRunning to is_running("Spotify")
set TVRunning to is_running("TV")

if QTRunning then
  tell application "QuickTime Player" to set isQTplaying to (playing of documents contains true)
else
  set isQTplaying to false
end if

if MusicRunning then
  tell application "Music" to set isMusicPlaying to (player state is playing)
else
  set isMusicPlaying to false
end if

if SpotifyRunning then
  tell application "Spotify" to set isSpotifyPlaying to (player state is playing)
else
  set isSpotifyPlaying to false
end if

if TVRunning then
  tell application "TV" to set isTVPlaying to (player state is playing)
else
  set isTVPlaying to false
end if

if isMusicPlaying or isSpotifyPlaying or isQTplaying or isTVPlaying then
  return "1"
else
  return "0"
end if
"#;

  let output = Command::new("osascript")
    .arg("-e")
    .arg(playing_music_state)
    .output()
    .await
    .expect("failed to run osascript");

  let playing = String::from_utf8(output.stdout).unwrap();

  Ok(playing.to_string())
}
