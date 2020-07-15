//! This file contains tests relevant to Rustup's handling of updating PATHs.
//! It depends on self-update working, so if absolutely everything here breaks,
//! check those tests as well.
pub mod mock;

use crate::mock::clitools::{self, expect_ok, Config, Scenario};
use crate::mock::{get_path, restore_path};
use lazy_static::lazy_static;
use std::path::PathBuf;
use std::sync::Mutex;

pub fn setup(f: &dyn Fn(&Config)) {
    clitools::setup(Scenario::SimpleV2, &|config| {
        // Lock protects environment variables
        lazy_static! {
            static ref LOCK: Mutex<()> = Mutex::new(());
        }
        let _g = LOCK.lock();

        // On windows these tests mess with the user's PATH. Save
        // and restore them here to keep from trashing things.
        let saved_path = get_path();
        let _g = scopeguard::guard(saved_path, restore_path);

        f(config);
    });
}

#[cfg(unix)]
mod unix {
    use super::*;
    use crate::mock::clitools::expect_err;
    use rustup::utils::raw;

    use std::fmt::Display;
    use std::fs;

    // Let's write a fake .rc which looks vaguely like a real script.
    const FAKE_RC: &str = r#"
# Sources fruity punch.
source ~/fruit/punch

# Adds apples to PATH.
export PATH="$HOME/apple/bin"
"#;

    const DEFAULT_EXPORT: &str = "export PATH=\"$HOME/.cargo/bin:$PATH\"";
    const POSIX_SH: &str = "env.sh";

    fn source(dir: impl Display, sh: impl Display) -> String {
        format!("source \"{dir}/{sh}\"", dir = dir, sh = sh)
    }

    fn cat(rc: impl Display, plus: impl Display) -> String {
        format!("{}\n{}\n", rc, plus)
    }

    #[test]
    fn install_creates_necessary_scripts() {
        setup(&|config| {
            // Override the test harness so that cargo home looks like
            // $HOME/.cargo by removing CARGO_HOME from the environment,
            // otherwise the literal path will be written to the file.

            let mut cmd = clitools::cmd(config, "rustup-init", &["-y"]);
            let files: Vec<PathBuf> = [".cargo/env.sh", ".profile", ".zshenv"]
                .iter()
                .map(|file| config.homedir.join(file))
                .collect();
            for file in &files {
                assert!(!file.exists());
            }
            cmd.env_remove("CARGO_HOME");
            cmd.env("SHELL", "zsh");
            assert!(cmd.output().unwrap().status.success());
            let mut rcs = files.iter();
            let env = rcs.next().unwrap();
            let envfile = fs::read_to_string(&env).unwrap();
            let (_, envfile_export) = envfile.split_at(match envfile.find("export PATH") {
                Some(idx) => idx,
                None => 0,
            });
            assert_eq!(&envfile_export[..DEFAULT_EXPORT.len()], DEFAULT_EXPORT);

            for rc in rcs {
                let expected = format!("\n{}\n", source("$HOME/.cargo", POSIX_SH));
                let new_profile = fs::read_to_string(&rc).unwrap();
                assert_eq!(new_profile, expected);
            }
        });
    }

    #[test]
    fn install_updates_bash_rcs() {
        setup(&|config| {
            let rcs: Vec<PathBuf> = [".bashrc", ".bash_profile", ".bash_login", ".profile"]
                .iter()
                .map(|rc| config.homedir.join(rc))
                .collect();
            for rc in &rcs {
                raw::write_file(&rc, FAKE_RC).unwrap();
            }

            expect_ok(config, &["rustup-init", "-y"]);

            let expected = cat(FAKE_RC, source(config.cargodir.display(), POSIX_SH));
            for rc in &rcs {
                let new_rc = fs::read_to_string(&rc).unwrap();
                assert_eq!(new_rc, expected);
            }
        })
    }

    #[test]
    fn install_does_not_create_bash_rcs() {
        setup(&|config| {
            let rcs: Vec<PathBuf> = [".bashrc", ".bash_profile", ".bash_login"]
                .iter()
                .map(|rc| config.homedir.join(rc))
                .collect();
            let rcs_before = rcs.iter().map(|rc| rc.exists());
            expect_ok(config, &["rustup-init", "-y"]);

            for (before, after) in rcs_before.zip(rcs.iter().map(|rc| rc.exists())) {
                assert!(before == false);
                assert_eq!(before, after);
            }
        });
    }

    #[test]
    fn install_errors_when_rc_cannot_be_updated() {
        setup(&|config| {
            let rc = config.homedir.join(".profile");
            fs::File::create(&rc).unwrap();
            let mut perms = fs::metadata(&rc).unwrap().permissions();
            perms.set_readonly(true);
            fs::set_permissions(&rc, perms).unwrap();

            expect_err(config, &["rustup-init", "-y"], "amend shell");
        });
    }

    #[test]
    fn install_with_zdotdir() {
        setup(&|config| {
            let zdotdir = tempfile::Builder::new()
                .prefix("zdotdir")
                .tempdir()
                .unwrap();
            let my_rc = "foo\nbar\nbaz";
            let rc = zdotdir.path().join(".zshenv");
            raw::write_file(&rc, my_rc).unwrap();

            let mut cmd = clitools::cmd(config, "rustup-init", &["-y"]);
            cmd.env("SHELL", "zsh");
            cmd.env("ZDOTDIR", zdotdir.path());
            assert!(cmd.output().unwrap().status.success());

            let new_rc = fs::read_to_string(&rc).unwrap();
            let addition = format!(r#"source "{}/env.sh""#, config.cargodir.display());
            let expected = format!("{}\n{}\n", my_rc, addition);
            assert_eq!(new_rc, expected);
        });
    }

    #[test]
    fn install_adds_path_to_rc_just_once() {
        setup(&|config| {
            let my_profile = "foo\nbar\nbaz";
            let profile = config.homedir.join(".profile");
            raw::write_file(&profile, my_profile).unwrap();
            expect_ok(config, &["rustup-init", "-y"]);
            expect_ok(config, &["rustup-init", "-y"]);

            let new_profile = fs::read_to_string(&profile).unwrap();
            let addition = format!(r#"source "{}/env.sh""#, config.cargodir.display());
            let expected = format!("{}\n{}\n", my_profile, addition);
            assert_eq!(new_profile, expected);
        });
    }

    #[test]
    fn uninstall_removes_source_from_rcs() {
        setup(&|config| {
            let rcs: Vec<PathBuf> = [
                ".bashrc",
                ".bash_profile",
                ".bash_login",
                ".profile",
                ".zshenv",
            ]
            .iter()
            .map(|rc| config.homedir.join(rc))
            .collect();
            for rc in &rcs {
                raw::write_file(&rc, FAKE_RC).unwrap();
            }

            expect_ok(config, &["rustup-init", "-y"]);
            expect_ok(config, &["rustup", "self", "uninstall", "-y"]);

            for rc in &rcs {
                let new_rc = fs::read_to_string(&rc).unwrap();
                assert_eq!(new_rc, FAKE_RC);
            }
        })
    }

    #[test]
    fn install_adds_sources_while_removing_legacy_paths() {
        setup(&|config| {
            let zdotdir = tempfile::Builder::new()
                .prefix("zdotdir")
                .tempdir()
                .unwrap();
            let rcs: Vec<PathBuf> = [".bash_profile", ".profile"]
                .iter()
                .map(|rc| config.homedir.join(rc))
                .collect();
            let zprofiles = vec![
                config.homedir.join(".zprofile"),
                zdotdir.path().join(".zprofile"),
            ];
            let old_rc = cat(FAKE_RC, DEFAULT_EXPORT);
            for rc in rcs.iter().chain(zprofiles.iter()) {
                raw::write_file(&rc, &old_rc).unwrap();
            }

            let mut cmd = clitools::cmd(config, "rustup-init", &["-y"]);
            cmd.env("SHELL", "zsh");
            cmd.env("ZDOTDIR", zdotdir.path());
            cmd.env_remove("CARGO_HOME");
            assert!(cmd.output().unwrap().status.success());
            let fixed_rc = cat(FAKE_RC, source("$HOME/.cargo", POSIX_SH));
            for rc in &rcs {
                let new_rc = fs::read_to_string(&rc).unwrap();
                assert_eq!(new_rc, fixed_rc);
            }
            for rc in &zprofiles {
                let new_rc = fs::read_to_string(&rc).unwrap();
                assert_eq!(new_rc, FAKE_RC);
            }
        })
    }

    #[test]
    fn uninstall_cleans_up_legacy_paths() {
        setup(&|config| {
            // Install first, then overwrite.
            expect_ok(config, &["rustup-init", "-y"]);

            let zdotdir = tempfile::Builder::new()
                .prefix("zdotdir")
                .tempdir()
                .unwrap();
            let mut cmd = clitools::cmd(config, "rustup-init", &["-y"]);
            cmd.env("SHELL", "zsh");
            cmd.env("ZDOTDIR", zdotdir.path());
            cmd.env_remove("CARGO_HOME");
            assert!(cmd.output().unwrap().status.success());
            let mut rcs: Vec<PathBuf> = [".bash_profile", ".profile", ".zprofile"]
                .iter()
                .map(|rc| config.homedir.join(rc))
                .collect();
            rcs.push(zdotdir.path().join(".zprofile"));
            let old_rc = cat(FAKE_RC, DEFAULT_EXPORT);
            for rc in &rcs {
                raw::write_file(&rc, &old_rc).unwrap();
            }

            let mut cmd = clitools::cmd(config, "rustup", &["self", "uninstall", "-y"]);
            cmd.env("SHELL", "zsh");
            cmd.env("ZDOTDIR", zdotdir.path());
            cmd.env_remove("CARGO_HOME");
            assert!(cmd.output().unwrap().status.success());

            for rc in &rcs {
                let new_rc = fs::read_to_string(&rc).unwrap();
                // It's not ideal, but it's OK, if we leave whitespace.
                assert_eq!(new_rc, FAKE_RC);
            }
        })
    }

    // In the default case we want to write $HOME/.cargo/bin as the path,
    // not the full path.
    #[test]
    fn when_cargo_home_is_the_default_write_path_specially() {
        setup(&|config| {
            // Override the test harness so that cargo home looks like
            // $HOME/.cargo by removing CARGO_HOME from the environment,
            // otherwise the literal path will be written to the file.

            let my_profile = "foo\nbar\nbaz";
            let profile = config.homedir.join(".profile");
            raw::write_file(&profile, my_profile).unwrap();
            let mut cmd = clitools::cmd(config, "rustup-init", &["-y"]);
            cmd.env_remove("CARGO_HOME");
            assert!(cmd.output().unwrap().status.success());

            let new_profile = fs::read_to_string(&profile).unwrap();
            let expected = format!("{}\nsource \"$HOME/.cargo/env.sh\"\n", my_profile);
            assert_eq!(new_profile, expected);

            let mut cmd = clitools::cmd(config, "rustup", &["self", "uninstall", "-y"]);
            cmd.env_remove("CARGO_HOME");
            assert!(cmd.output().unwrap().status.success());

            let new_profile = fs::read_to_string(&profile).unwrap();
            assert_eq!(new_profile, my_profile);
        });
    }

    #[test]
    fn install_doesnt_modify_path_if_passed_no_modify_path() {
        setup(&|config| {
            let profile = config.homedir.join(".profile");
            expect_ok(config, &["rustup-init", "-y", "--no-modify-path"]);
            assert!(!profile.exists());
        });
    }
}

#[cfg(windows)]
mod windows {
    use super::*;

    #[test]
    fn install_doesnt_modify_path_if_passed_no_modify_path() {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
        use winreg::RegKey;

        setup(&|config| {
            let root = RegKey::predef(HKEY_CURRENT_USER);
            let environment = root
                .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
                .unwrap();
            let old_path = environment.get_raw_value("PATH").unwrap();

            expect_ok(config, &["rustup-init", "-y", "--no-modify-path"]);

            let root = RegKey::predef(HKEY_CURRENT_USER);
            let environment = root
                .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
                .unwrap();
            let new_path = environment.get_raw_value("PATH").unwrap();

            assert_eq!(old_path, new_path);
        });
    }

    // HKCU\Environment\PATH may not exist during install, and it may need to be
    // deleted during uninstall if we remove the last path from it
    #[test]
    fn windows_handle_empty_path_registry_key() {
        use winreg::enums::{RegType, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
        use winreg::RegKey;

        setup(&|config| {
            let root = RegKey::predef(HKEY_CURRENT_USER);
            let environment = root
                .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
                .unwrap();
            let _ = environment.delete_value("PATH");

            expect_ok(config, &["rustup-init", "-y"]);

            let root = RegKey::predef(HKEY_CURRENT_USER);
            let environment = root
                .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
                .unwrap();
            let path = environment.get_raw_value("PATH").unwrap();
            assert!(path.vtype == RegType::REG_EXPAND_SZ);

            expect_ok(config, &["rustup", "self", "uninstall", "-y"]);

            let root = RegKey::predef(HKEY_CURRENT_USER);
            let environment = root
                .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
                .unwrap();
            let path = environment.get_raw_value("PATH");

            assert!(path.is_err());
        });
    }

    #[test]
    fn uninstall_removes_path() {
        setup(&|config| {
            expect_ok(config, &["rustup-init", "-y"]);
            expect_ok(config, &["rustup", "self", "uninstall", "-y"]);

            let path = config.cargodir.join("bin").to_string_lossy().to_string();
            assert!(!get_path().unwrap().contains(&path));
        });
    }

    #[test]
    fn install_adds_path() {
        setup(&|config| {
            expect_ok(config, &["rustup-init", "-y"]);

            let path = config.cargodir.join("bin").to_string_lossy().to_string();
            assert!(
                get_path().unwrap().contains(&path),
                format!("`{}` not in `{}`", get_path().unwrap(), &path)
            );
        });
    }
}
