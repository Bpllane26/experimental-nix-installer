use nix::unistd::User;
use target_lexicon::OperatingSystem;
use tokio::process::Command;
use tracing::{span, Span};

use crate::action::{ActionError, ActionErrorKind, ActionTag};
use crate::execute_command;

use crate::action::{Action, ActionDescription, StatefulAction};

/**
Delete an operating system level user
*/
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
pub struct DeleteUser {
    name: String,
}

impl DeleteUser {
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn plan(name: String) -> Result<StatefulAction<Self>, ActionError> {
        let this = Self { name: name.clone() };

        match OperatingSystem::host() {
            OperatingSystem::MacOSX { .. } | OperatingSystem::Darwin => (),
            _ => {
                if !(which::which("userdel").is_ok() || which::which("deluser").is_ok()) {
                    return Err(Self::error(ActionErrorKind::MissingUserDeletionCommand));
                }
            },
        }

        // Ensure user exists
        let _ = User::from_name(name.as_str())
            .map_err(|e| ActionErrorKind::GettingUserId(name.clone(), e))
            .map_err(Self::error)?
            .ok_or_else(|| ActionErrorKind::NoUser(name.clone()))
            .map_err(Self::error)?;

        // There is no "StatefulAction::completed" for this action since if the user is to be deleted
        // it is an error if it does not exist.

        Ok(StatefulAction::uncompleted(this))
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "delete_user")]
impl Action for DeleteUser {
    fn action_tag() -> ActionTag {
        ActionTag("delete_user")
    }
    fn tracing_synopsis(&self) -> String {
        format!(
            "Delete user `{}`, which exists due to a previous install, but is no longer required",
            self.name
        )
    }

    fn tracing_span(&self) -> Span {
        span!(tracing::Level::DEBUG, "delete_user", user = self.name,)
    }

    fn execute_description(&self) -> Vec<ActionDescription> {
        vec![ActionDescription::new(
            self.tracing_synopsis(),
            vec![format!(
                "Nix with `auto-allocate-uids = true` no longer requires explicitly created users, so this user can be removed"
            )],
        )]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(&mut self) -> Result<(), ActionError> {
        use OperatingSystem;
        match OperatingSystem::host() {
            OperatingSystem::MacOSX {
                major: _,
                minor: _,
                patch: _,
            }
            | OperatingSystem::Darwin => {
                // MacOS is a "Special" case
                // It's only possible to delete users under certain conditions.
                // Documentation on https://it.megocollector.com/macos/cant-delete-a-macos-user-with-dscl-resolution/ and http://www.aixperts.co.uk/?p=214 suggested it was a secure token
                // That is correct, however it's a bit more nuanced. It appears to be that a user must be graphically logged in for some other user on the system to be deleted.
                let mut command = Command::new("/usr/bin/dscl");
                command.args([".", "-delete", &format!("/Users/{}", self.name)]);
                command.process_group(0);
                command.stdin(std::process::Stdio::null());

                let output = command
                    .output()
                    .await
                    .map_err(|e| ActionErrorKind::command(&command, e))
                    .map_err(Self::error)?;
                let stderr = String::from_utf8_lossy(&output.stderr);
                match output.status.code() {
                    Some(0) => (),
                    Some(40) if stderr.contains("-14120") => {
                        // The user is on an ephemeral Mac, like detsys uses
                        // These Macs cannot always delete users, as sometimes there is no graphical login
                        tracing::warn!("Encountered an exit code 40 with -14120 error while removing user, this is likely because the initial executing user did not have a secure token, or that there was no graphical login session. To delete the user, log in graphically, then run `/usr/bin/dscl . -delete /Users/{}", self.name);
                    },
                    _ => {
                        // Something went wrong
                        return Err(Self::error(ActionErrorKind::command_output(
                            &command, output,
                        )));
                    },
                }
            },
            _ => {
                if which::which("userdel").is_ok() {
                    execute_command(
                        Command::new("userdel")
                            .process_group(0)
                            .arg(&self.name)
                            .stdin(std::process::Stdio::null()),
                    )
                    .await
                    .map_err(Self::error)?;
                } else if which::which("deluser").is_ok() {
                    execute_command(
                        Command::new("deluser")
                            .process_group(0)
                            .arg(&self.name)
                            .stdin(std::process::Stdio::null()),
                    )
                    .await
                    .map_err(Self::error)?;
                } else {
                    return Err(Self::error(ActionErrorKind::MissingUserDeletionCommand));
                }
            },
        };

        Ok(())
    }

    fn revert_description(&self) -> Vec<ActionDescription> {
        vec![]
    }

    #[tracing::instrument(level = "debug", skip_all)]
    async fn revert(&mut self) -> Result<(), ActionError> {
        Ok(())
    }
}
