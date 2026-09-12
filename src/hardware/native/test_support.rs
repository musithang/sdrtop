// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) struct TestLibrary {
    path: PathBuf,
    lib: libloading::Library,
}

impl TestLibrary {
    pub fn new(omit_symbol: bool) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sdrtop-native-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("libfixture.so");
        let mut cc = Command::new("cc");
        cc.args(["-shared", "-fPIC", "-std=c11", "-Wall", "-Werror"]);
        if omit_symbol {
            cc.arg("-DOMIT_LAST_SYMBOL");
        }
        let output = cc
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/hardware/native/fixtures/library.c"
            ))
            .arg("-o")
            .arg(&path)
            .output()
            .expect("the native loader fixture requires the C compiler used to link sdrtop");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let lib = unsafe { libloading::Library::new(&path) }.unwrap();
        Self { path, lib }
    }

    pub fn library(&self) -> libloading::Library {
        unsafe { libloading::Library::new(&self.path) }.unwrap()
    }

    pub fn path(&self) -> &str {
        self.path.to_str().unwrap()
    }

    pub fn mode(&self, mode: i32) {
        unsafe {
            self.lib
                .get::<unsafe extern "C" fn(i32)>(b"fixture_mode\0")
                .unwrap()(mode);
        }
    }

    pub fn calls(&self, slot: i32) -> i32 {
        unsafe {
            self.lib
                .get::<unsafe extern "C" fn(i32) -> i32>(b"fixture_calls\0")
                .unwrap()(slot)
        }
    }

    pub fn list_layout(&self, field: i32) -> usize {
        unsafe {
            self.lib
                .get::<unsafe extern "C" fn(i32) -> usize>(b"fixture_list_layout\0")
                .unwrap()(field)
        }
    }
}

impl Drop for TestLibrary {
    fn drop(&mut self) {
        let result = std::fs::remove_file(&self.path)
            .and_then(|()| std::fs::remove_dir(self.path.parent().unwrap()));
        if let Err(err) = result {
            if std::thread::panicking() {
                eprintln!(
                    "Could not clean up native fixture {}: {err}",
                    self.path.display()
                );
            } else {
                panic!(
                    "Could not clean up native fixture {}: {err}",
                    self.path.display()
                );
            }
        }
    }
}
