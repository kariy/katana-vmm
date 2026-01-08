use crate::state::StateDatabase;
use anyhow::Result;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};

pub fn execute(name: &str, follow: bool, tail: usize, db: &StateDatabase) -> Result<()> {
    // Load instance from database
    let state = db.get_instance(name)?;

    let log_path = state
        .serial_log
        .ok_or_else(|| anyhow::anyhow!("Instance has no serial log"))?;

    if !log_path.exists() {
        anyhow::bail!(
            "Log file not found: {}. Instance may not have been started yet.",
            log_path.display()
        );
    }

    if follow {
        // Follow mode (tail -f)
        println!("Following logs from {} (Ctrl+C to exit)...\n", log_path.display());

        let mut file = File::open(&log_path)?;

        // Seek to end minus tail lines
        if tail > 0 {
            // Read last N lines first
            file.seek(SeekFrom::Start(0))?;
            let reader = BufReader::new(&file);
            let lines: Vec<String> = reader.lines().collect::<Result<_, _>>()?;

            let start_idx = if lines.len() > tail {
                lines.len() - tail
            } else {
                0
            };

            for line in &lines[start_idx..] {
                println!("{}", line);
            }
        }

        // Now follow new lines
        let mut file = File::open(&log_path)?;
        file.seek(SeekFrom::End(0))?;
        let mut reader = BufReader::new(file);

        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    // No new data, sleep a bit
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    continue;
                }
                Ok(_) => {
                    print!("{}", line);
                }
                Err(e) => {
                    eprintln!("Error reading log: {}", e);
                    break;
                }
            }
        }
    } else {
        // Just show last N lines
        let file = File::open(&log_path)?;
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader.lines().collect::<Result<_, _>>()?;

        let start_idx = if lines.len() > tail {
            lines.len() - tail
        } else {
            0
        };

        for line in &lines[start_idx..] {
            println!("{}", line);
        }
    }

    Ok(())
}
