pub mod canonical;
pub mod clash_yaml;
pub mod detect;
pub mod duration;
pub mod error;
pub mod model;
pub mod render;
pub mod protocol;
pub mod score;
pub mod uri;

pub use detect::{Format, ParsedSubscription, detect, parse_subscription};
pub use duration::parse_duration;
pub use error::Error;
