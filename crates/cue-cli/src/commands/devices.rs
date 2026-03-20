use anyhow::Result;
use cue_core::audio::AudioCapture;
#[cfg(target_os = "linux")]
use cue_core::audio::SystemAudioCapture;

pub fn run() -> Result<()> {
    let devices = AudioCapture::list_devices()?;

    if devices.is_empty() {
        println!("No audio devices found.");
        return Ok(());
    }

    println!(
        "{:<4} {:<8} {:<8} {:<6} {:<12} {}",
        "IDX", "INPUT", "OUTPUT", "CH", "RATE", "NAME"
    );
    println!("{}", "-".repeat(60));

    for (i, device) in devices.iter().enumerate() {
        let input_marker = if device.is_default_input { "* " } else { "  " };
        let output_marker = if device.is_default_output { "* " } else { "  " };
        println!(
            "{:<4} {:<8} {:<8} {:<6} {:<12} {}",
            i,
            input_marker,
            output_marker,
            device.channels,
            device.sample_rate,
            device.name
        );
    }

    println!("\n* = default device");

    // Show PipeWire monitor sources (Linux only)
    #[cfg(target_os = "linux")]
    {
        println!("\n--- PipeWire Monitor Sources (for --system-audio) ---");
        if SystemAudioCapture::is_available() {
            match SystemAudioCapture::find_monitor_device() {
                Some(monitor) => {
                    println!("pw-record available  [OK]");
                    println!("Default monitor source: {}", monitor);
                }
                None => {
                    println!("pw-record available  [OK]");
                    println!("No monitor source found via pactl (may still work with --system-audio)");
                }
            }
        } else {
            println!(
                "pw-record not found. Install with: sudo apt install pipewire-audio-client-libraries"
            );
            println!("System audio capture (--system-audio) requires pw-record.");
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        println!("\nNote: System audio capture (--system-audio) is only available on Linux.");
    }

    Ok(())
}
