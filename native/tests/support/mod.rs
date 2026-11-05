use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

static ENVIRONMENT: Mutex<()> = Mutex::new(());

pub struct TestWorkspace {
    pub root: PathBuf,
    previous_root: Option<std::ffi::OsString>,
    previous_home: Option<std::ffi::OsString>,
    previous_userprofile: Option<std::ffi::OsString>,
    previous_codex_home: Option<std::ffi::OsString>,
    _guard: MutexGuard<'static, ()>,
}

impl TestWorkspace {
    pub fn new(tag: &str) -> Self {
        let guard = ENVIRONMENT.lock().unwrap_or_else(|error| error.into_inner());
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("easel-{tag}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(root.join("outputs")).unwrap();
        fs::create_dir_all(root.join("skills")).unwrap();
        fs::write(root.join("skills/skills-manifest.json"), r#"{"version":2,"skills":[]}"#).unwrap();
        let previous_root = std::env::var_os("OTOOLS_EASEL_ROOT");
        let previous_home = std::env::var_os("HOME");
        let previous_userprofile = std::env::var_os("USERPROFILE");
        let previous_codex_home = std::env::var_os("CODEX_HOME");
        fs::create_dir_all(root.join("home/.codex")).unwrap();
        std::env::set_var("OTOOLS_EASEL_ROOT", &root);
        std::env::set_var("HOME", root.join("home"));
        std::env::set_var("USERPROFILE", root.join("home"));
        std::env::set_var("CODEX_HOME", root.join("home/.codex"));
        Self { root, previous_root, previous_home, previous_userprofile, previous_codex_home, _guard: guard }
    }

    pub fn write(&self, path: &str, contents: impl AsRef<[u8]>) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        if let Some(value) = &self.previous_root {
            std::env::set_var("OTOOLS_EASEL_ROOT", value);
        } else {
            std::env::remove_var("OTOOLS_EASEL_ROOT");
        }
        for (key, previous) in [
            ("HOME", &self.previous_home),
            ("USERPROFILE", &self.previous_userprofile),
            ("CODEX_HOME", &self.previous_codex_home),
        ] {
            if let Some(value) = previous { std::env::set_var(key, value); } else { std::env::remove_var(key); }
        }
        fs::remove_dir_all(&self.root).unwrap();
    }
}
