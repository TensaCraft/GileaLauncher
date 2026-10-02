//! A stand-in for Java running installer processors: it behaves like the fake installer's patcher
//! (reads `--clean` and `--apply`, writes `--slim` and `--output`) and records every call.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use launcher_core::loaders::processors::{ProcessorCall, ProcessorRunner};

use super::fake_mojang::PATCHED_BYTES;

#[derive(Default)]
pub struct FakeRunner {
    pub calls: Mutex<Vec<ProcessorCall>>,
    /// The next runs fail as a crashing processor would.
    pub fail: AtomicBool,
    /// The next runs write the wrong bytes to `--output`.
    pub wrong_output: AtomicBool,
}

impl FakeRunner {
    pub fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

fn value<'a>(call: &'a ProcessorCall, flag: &str) -> Option<&'a str> {
    let at = call.args.iter().position(|arg| arg == flag)?;
    call.args.get(at + 1).map(String::as_str)
}

impl ProcessorRunner for FakeRunner {
    fn run(&self, java: &Path, call: &ProcessorCall, cwd: &Path) -> Result<(), String> {
        self.calls.lock().unwrap().push(call.clone());
        if self.fail.load(Ordering::SeqCst) {
            return Err("processor exploded".into());
        }
        let missing: Vec<String> = [java.to_path_buf(), cwd.to_path_buf()]
            .into_iter()
            .chain(call.classpath.iter().cloned())
            .chain(["--clean", "--apply"].iter().filter_map(|flag| value(call, flag)).map(PathBuf::from))
            .filter(|path| !path.exists())
            .map(|path| path.display().to_string())
            .collect();
        if !missing.is_empty() {
            return Err(format!("missing inputs: {}", missing.join(", ")));
        }
        let patched =
            if self.wrong_output.load(Ordering::SeqCst) { b"wrong".as_slice() } else { PATCHED_BYTES };
        for (flag, bytes) in [("--slim", b"slim".as_slice()), ("--output", patched)] {
            if let Some(path) = value(call, flag) {
                let path = Path::new(path);
                std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
                std::fs::write(path, bytes).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}
