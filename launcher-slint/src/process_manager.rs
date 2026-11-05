#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

#[allow(dead_code)]
pub struct ManagedProcess {
    child: Arc<Mutex<Option<Child>>>,
}

impl ManagedProcess {
    pub fn new() -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
        }
    }

    pub fn is_running(&self) -> bool {
        if let Ok(mut lock) = self.child.lock() {
            if let Some(child) = lock.as_mut() {
                match child.try_wait() {
                    Ok(Some(_)) => {
                        *lock = None;
                        false
                    }
                    Ok(None) => true,
                    Err(_) => false,
                }
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn start<F: Fn(String) + Send + Sync + 'static>(
        &self,
        mut cmd: Command,
        on_line: F,
        on_exit: impl Fn() + Send + 'static,
    ) -> Result<(), String> {
        if self.is_running() {
            return Err("Процесс уже запущен".to_string());
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Не удалось запустить процесс: {}", e))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        if let Ok(mut lock) = self.child.lock() {
            *lock = Some(child);
        }

        let child_arc = self.child.clone();

        // Spawn stdout reader
        let on_line_arc = Arc::new(on_line);
        let on_line_stdout = on_line_arc.clone();

        if let Some(out) = stdout {
            thread::spawn(move || {
                let reader = BufReader::new(out);
                for line in reader.lines().flatten() {
                    on_line_stdout(line);
                }
            });
        }

        // Spawn stderr reader
        let on_line_stderr = on_line_arc.clone();
        if let Some(err) = stderr {
            thread::spawn(move || {
                let reader = BufReader::new(err);
                for line in reader.lines().flatten() {
                    on_line_stderr(line);
                }
            });
        }

        // Spawn exit watcher
        thread::spawn(move || {
            loop {
                thread::sleep(std::time::Duration::from_millis(500));
                let mut finished = false;
                if let Ok(mut lock) = child_arc.lock() {
                    if let Some(child) = lock.as_mut() {
                        if let Ok(Some(_)) = child.try_wait() {
                            finished = true;
                            *lock = None;
                        }
                    } else {
                        finished = true;
                    }
                }
                if finished {
                    on_exit();
                    break;
                }
            }
        });

        Ok(())
    }

    pub fn stop(&self) {
        if let Ok(mut lock) = self.child.lock() {
            if let Some(child) = lock.as_mut() {
                let pid = child.id();
                if cfg!(windows) {
                    let _ = Command::new("taskkill")
                        .args(["/F", "/T", "/PID", &pid.to_string()])
                        .output();
                } else {
                    let _ = child.kill();
                }
            }
            *lock = None;
        }
    }
}
