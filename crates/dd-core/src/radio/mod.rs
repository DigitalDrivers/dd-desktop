//! Team radio ("Boxenfunk"): the coach speaks while the driver drives. Everything here is pure logic on what the
//! game publishes in its shared memory; the shell copies the pages, plays the sound and speaks.

pub mod effect;
pub mod frame;
pub mod queue;
pub mod settings;
pub mod spotter;
#[cfg(test)]
mod testkit;
