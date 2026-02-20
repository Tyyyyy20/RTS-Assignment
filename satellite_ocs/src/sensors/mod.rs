pub mod thermal;
pub mod power;
pub mod attitude;

use crate::config::Config;

pub async fn spawn_all(_cfg: Config) {
    thermal::spawn();
    power::spawn();
    attitude::spawn();
}
