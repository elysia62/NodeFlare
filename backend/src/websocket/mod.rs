pub mod agent;
pub mod dashboard;

use tokio::sync::mpsc;

#[derive(Clone)]
pub struct AgentConnection {
    pub connection_id: String,
    pub sender: mpsc::Sender<AgentCommand>,
    pub report_interval: i64,
    pub collect_interval: i64,
}

#[derive(Debug)]
pub enum AgentCommand {
    Text(String),
    Pong(Vec<u8>),
    Close,
}

#[derive(Debug, Clone)]
pub struct DashboardEvent {
    pub server_id: String,
    pub payload: String,
}
