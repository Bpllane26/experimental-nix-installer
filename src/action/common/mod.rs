//! [`Action`](crate::action::Action)s which only call other base plugins

pub(crate) mod configure_init_service;
pub(crate) mod configure_nix;
pub(crate) mod configure_shell_profile;
pub(crate) mod create_nix_tree;
pub(crate) mod delete_users;
pub(crate) mod place_nix_configuration;
pub(crate) mod provision_nix;

pub use configure_init_service::{ConfigureInitService, ConfigureNixDaemonServiceError};
pub use configure_nix::ConfigureNix;
pub use configure_shell_profile::ConfigureShellProfile;
pub use create_nix_tree::CreateNixTree;
pub use delete_users::DeleteUsersInGroup;
pub use place_nix_configuration::PlaceNixConfiguration;
pub use provision_nix::ProvisionNix;
