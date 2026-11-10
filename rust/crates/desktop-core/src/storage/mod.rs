//! Persistence layer replacing Dexie, localStorage and keytar.

pub mod json_store;
pub mod paths;
pub mod secure_store;

pub use json_store::Storage;
pub use paths::AppPaths;
pub use secure_store::{SecureStore, base64_decode, base64_encode};
