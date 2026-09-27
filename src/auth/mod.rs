//! Authentication module for Telmoni CLI using RFC 8628 device flow and API tokens.

pub mod device;
pub mod storage;

pub use device::{
    AuthnResult, DeviceStart, Me, Person, PollOutcome, Team, fetch_me, poll_once,
    poll_until_granted, refresh_if_needed, refresh_tokens, revoke_session, start_device_auth,
};
pub use storage::{AuthType, Credentials, CredentialsStore, StoredPerson, StoredTeam};
