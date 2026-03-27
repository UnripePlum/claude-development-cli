use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
pub struct Session {
    pub name: String,
    pub workers: Vec<WorkerInfo>,
    pub created_at: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WorkerInfo {
    pub name: String,
    pub cwd: Option<String>,
}

fn sessions_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cdc")
        .join("sessions")
}

fn archives_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cdc")
        .join("archives")
}

pub fn save_session(session: &Session) -> Result<(), Box<dyn std::error::Error>> {
    let dir = sessions_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", session.name));
    let json = serde_json::to_string_pretty(session)?;
    fs::write(&path, json)?;
    Ok(())
}

pub fn load_session(name: &str) -> Result<Session, Box<dyn std::error::Error>> {
    let path = sessions_dir().join(format!("{}.json", name));
    let json = fs::read_to_string(&path)?;
    let session: Session = serde_json::from_str(&json)?;
    Ok(session)
}

pub fn list_sessions() -> Vec<String> {
    let dir = sessions_dir();
    let mut names = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if let Some(stem) = name.strip_suffix(".json") {
                    names.push(stem.to_string());
                }
            }
        }
    }
    names.sort();
    names
}

pub fn archive_session(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let src = sessions_dir().join(format!("{}.json", name));
    if !src.exists() {
        return Err(format!("Session '{}' not found", name).into());
    }
    let dst_dir = archives_dir();
    fs::create_dir_all(&dst_dir)?;
    let dst = dst_dir.join(format!("{}.json", name));
    fs::rename(&src, &dst)?;
    Ok(())
}

pub fn delete_session(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = sessions_dir().join(format!("{}.json", name));
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}
