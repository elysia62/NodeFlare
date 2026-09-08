pub mod agent;
pub mod dashboard;
pub(crate) mod ingest;

use tokio::sync::mpsc;

#[derive(Clone)]
pub struct AgentConnection {
    pub connection_id: String,
    pub sender: mpsc::Sender<AgentCommand>,
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
    pub payload: axum::body::Bytes,
}
