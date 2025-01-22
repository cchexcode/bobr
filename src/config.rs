#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Config {
    pub commands: Vec<Command>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Command {
    pub command: String,
}
