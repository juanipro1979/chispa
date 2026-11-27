//! Chispa core: el motor (libobs) y el bus de comandos.
//! La UI, los atajos y (más adelante) la voz mandan todos los mismos `Command` al motor.

pub mod audio;
pub mod bus;
pub mod engine;
pub mod live;
pub mod profile;
pub mod vertical;

pub use bus::{Command, EngineHandle, Status};
pub use engine::{EngineOptions, SceneMode, StreamTarget};
