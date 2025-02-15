use crate::{
    action::{
        macos::NIX_VOLUME_MOUNTD_DEST, Action, ActionDescription, ActionError, ActionErrorKind,
        ActionState, ActionTag, StatefulAction,
    },
    execute_command,
    os::darwin::DiskUtilApfsListOutput,
};
use rand::Rng;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::process::Command;
use tracing::{span, Span};

use super::CreateApfsVolume;

/**
Encrypt an APFS volume
 */
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
pub struct EncryptApfsVolume {
    disk: PathBuf,
    name: String,
}

impl EncryptApfsVolume {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(
        disk: impl AsRef<Path>,
        name: impl AsRef<str>,
        planned_create_apfs_volume: &StatefulAction<CreateApfsVolume>,
    ) -> Result<StatefulAction<Self>, ActionError> {
        let name = name.as_ref().to_owned();
        let disk = disk.as_ref().to_path_buf();

        let mut command = Command::new("/usr/bin/security");
        command.args(["find-generic-password", "-a"]);
        command.arg(&name);
        command.arg("-s");
        command.arg("Nix Store");
        command.arg("-l");
        command.arg(&format!("{} encryption password", disk.display()));
        command.arg("-D");
        command.arg("Encrypted volume password");
        command.process_group(0);
        command.stdin(Stdio::null());
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
        if command
            .status()
            .await
            .map_err(|e| Self::error(ActionErrorKind::command(&command, e)))?
            .success()
        {
            // The user has a password matching what we would create.
            if planned_create_apfs_volume.state == ActionState::Completed {
                // We detected a created volume already, and a password exists, so we can keep using that and skip doing anything
                return Ok(StatefulAction::completed(Self { name, disk }));
            }

            // Ask the user to remove it
            return Err(Self::error(EncryptApfsVolumeError::ExistingPasswordFound(
                name, disk,
            )));
        } else {
            if planned_create_apfs_volume.state == ActionState::Completed {
                // The user has a volume already created, but a password not set. This means we probably can't decrypt the volume.
                return Err(Self::error(
                    EncryptApfsVolumeError::MissingPasswordForExistingVolume(name, disk),
                ));
            }
        }

        // Ensure if the disk already exists, that it's encrypted
        let output =
            execute_command(Command::new("/usr/sbin/diskutil").args(["apfs", "list", "-plist"]))
                .await
                .map_err(Self::error)?;

        let parsed: DiskUtilApfsListOutput =
            plist::from_bytes(&output.stdout).map_err(Self::error)?;
        for container in parsed.containers {
            for volume in container.volumes {
                if volume.name == name {
                    match volume.encryption == false {
                        true => {
                            return Ok(StatefulAction::completed(Self { disk, name }));
                        },
                        false => {
                            return Err(Self::error(
                                EncryptApfsVolumeError::ExistingVolumeNotEncrypted(name, disk),
                            ));
                        },
                    }
                }
            }
        }

        Ok(StatefulAction::uncompleted(Self { name, disk }))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "encrypt_volume")]
impl Action for EncryptApfsVolume {
    fn action_tag() -> ActionTag {
        ActionTag("encrypt_apfs_volume")
    }
    fn tracing_synopsis(&self) -> String {
        format!(
            "Encrypt volume `{}` on disk `{}`",
            self.name,
            self.disk.display()
        )
    }

    fn tracing_span(&self) -> Span {
        span!(
            tracing::Level::DEBUG,
            "encrypt_volume",
            disk = tracing::field::display(self.disk.display()),
        )
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(self.tracing_synopsis(), vec![])]
    }

    #[tracing::instrument(level = "debug", skip_all, fields(
        disk = %self.disk.display(),
    ))]
    async fn execute(&mut self) -> Result<(), ActionError> {
        let Self { disk, name } = self;

        // Generate a random password.
        let password: String = {
            const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ\
                                abcdefghijklmnopqrstuvwxyz\
                                    0123456789)(*&^%$#@!~";
            const PASSWORD_LEN: usize = 32;
            let mut rng = rand::thread_rng();

            (0..PASSWORD_LEN)
                .map(|_| {
                    let idx = rng.gen_range(0..CHARSET.len());
                    CHARSET[idx] as char
                })
                .collect()
        };

        let disk_str = disk.to_str().expect("Could not turn disk into string"); /* Should not reasonably ever fail */

        execute_command(Command::new("/usr/sbin/diskutil").arg("mount").arg(&name))
            .await
            .map_err(Self::error)?;

        // Add the password to the user keychain so they can unlock it later.
        execute_command(
            Command::new("/usr/bin/security").process_group(0).args([
                "add-generic-password",
                "-a",
                name.as_str(),
                "-s",
                "Nix Store",
                "-l",
                format!("{} encryption password", disk_str).as_str(),
                "-D",
                "Encrypted volume password",
                "-j",
                format!(
                    "Added automatically by the Nix installer for use by {NIX_VOLUME_MOUNTD_DEST}"
                )
                .as_str(),
                "-w",
                password.as_str(),
                "-T",
                "/System/Library/CoreServices/APFSUserAgent",
                "-T",
                "/System/Library/CoreServices/CSUserAgent",
                "-T",
                "/usr/bin/security",
                "/Library/Keychains/System.keychain",
            ]),
        )
        .await
        .map_err(Self::error)?;

        // Encrypt the mounted volume
        execute_command(Command::new("/usr/sbin/diskutil").process_group(0).args([
            "apfs",
            "encryptVolume",
            name.as_str(),
            "-user",
            "disk",
            "-passphrase",
            password.as_str(),
        ]))
        .await
        .map_err(Self::error)?;

        execute_command(
            Command::new("/usr/sbin/diskutil")
                .process_group(0)
                .arg("unmount")
                .arg("force")
                .arg(&name),
        )
        .await
        .map_err(Self::error)?;

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            format!(
                "Remove encryption keys for volume `{}`",
                self.disk.display()
            ),
            vec![],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all, fields(
        disk = %self.disk.display(),
    ))]
    async fn revert(&mut self) -> Result<(), ActionError> {
        let disk_str = self.disk.to_str().expect("Could not turn disk into string"); /* Should not reasonably ever fail */

        // TODO: This seems very rough and unsafe
        execute_command(
            Command::new("/usr/bin/security").process_group(0).args([
                "delete-generic-password",
                "-a",
                self.name.as_str(),
                "-s",
                self.name.as_str(),
                "-l",
                format!("{} encryption password", disk_str).as_str(),
                "-D",
                "Encrypted volume password",
                "-j",
                format!(
                    "Added automatically by the Nix installer for use by {NIX_VOLUME_MOUNTD_DEST}"
                )
                .as_str(),
            ]),
        )
        .await
        .map_err(Self::error)?;

        Ok(())
    }
}

#[derive(thiserror::Error, Debug)]
pub enum EncryptApfsVolumeError {
    #[error("The keychain has an existing password for a non-existing \"{0}\" volume on disk `{1}`, consider removing the password with `security delete-generic-password  -a \"{0}\" -s \"Nix Store\" -l \"{1} encryption password\" -D \"Encrypted volume password\"`")]
    ExistingPasswordFound(String, PathBuf),
    #[error("The keychain lacks a password for the already existing \"{0}\" volume on disk `{1}`, consider removing the volume with `diskutil apfs deleteVolume \"{0}\"` (if you receive error -69888, you may need to run `launchctl bootout system/org.nixos.darwin-store` and `launchctl bootout system/org.nixos.nix-daemon` first)")]
    MissingPasswordForExistingVolume(String, PathBuf),
    #[error("The existing APFS volume \"{0}\" on disk `{1}` is not encrypted but it should be, consider removing the volume with `diskutil apfs deleteVolume \"{0}\"` (if you receive error -69888, you may need to run `launchctl bootout system/org.nixos.darwin-store` and `launchctl bootout system/org.nixos.nix-daemon` first)")]
    ExistingVolumeNotEncrypted(String, PathBuf),
}

impl Into<ActionErrorKind> for EncryptApfsVolumeError {
    fn into(self) -> ActionErrorKind {
        ActionErrorKind::Custom(Box::new(self))
    }
}
