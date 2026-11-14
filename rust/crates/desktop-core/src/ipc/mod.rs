//! The the legacy shell IPC contract, reimplemented for wry.
//!
//! `channels` is a literal transcription of `ipc-shared.ts`; `protocol`
//! defines the wire frames and the [`Host`](protocol::Host) seam that keeps
//! this crate independent of tao/wry.

pub mod channels;
pub mod protocol;

pub use channels::{event, invoke, send};
pub use protocol::{
    FileFilter, Host, IpcMessage, MenuItemState, MenuStateUpdate, MessageOptions, NullHost,
    OpenDialogOptions, RpcResult, SaveDialogOptions, SerializableMenuItem, WindowAction,
    WindowQuery,
};
