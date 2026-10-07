use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(untagged)]
pub enum MolangExpression {
    #[default]
    Null,
    ValueExpression(Value),
}
