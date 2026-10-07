use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct WorldPolicies {}

impl WorldPolicies {
    pub fn new() -> Self {
        Self {}
    }
}
