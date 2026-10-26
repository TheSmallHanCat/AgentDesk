use crate::{
    error::{AppError, AppResult},
    models::{now, Connection, Project, Target},
};
use rusqlite::{params, OptionalExtension};
use std::{
    path::Path,
    sync::{Mutex, MutexGuard},
};

pub struct Db {
    conn: Mutex<rusqlite::Connection>,
}
fn db_error(e: impl std::fmt::Display) -> AppError {
    AppError::new("E_STORAGE", "database", e.to_string())
}
impl Db {
    pub fn open(path: &Path) -> AppResult<Self> {
        let mut conn = rusqlite::Connection::open(path).map_err(db_error)?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(db_error)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(db_error)?;
        let version: u32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db_error)?;
        if version > 1 {
            return Err(AppError::new(
                "E_DB_VERSION",
                "database",
                "Database requires a newer application",
            ));
        }
        if version == 0 {
            let tx = conn.transaction().map_err(db_error)?;
            tx.execute_batch("CREATE TABLE connections(id TEXT PRIMARY KEY, data TEXT NOT NULL);
                CREATE TABLE projects(id TEXT PRIMARY KEY, connection_id TEXT NOT NULL REFERENCES connections(id), data TEXT NOT NULL);
                CREATE TABLE recent_dirs(connection_id TEXT NOT NULL REFERENCES connections(id) ON DELETE CASCADE, remote_user TEXT NOT NULL, path TEXT NOT NULL, used_at INTEGER NOT NULL, PRIMARY KEY(connection_id,remote_user,path));
                PRAGMA user_version=1;").map_err(db_error)?;
            tx.commit().map_err(db_error)?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
    fn lock(&self) -> AppResult<MutexGuard<'_, rusqlite::Connection>> {
        self.conn.lock().map_err(db_error)
    }
    pub fn connections(&self) -> AppResult<Vec<Connection>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare("SELECT data FROM connections ORDER BY id")
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db_error)?;
        rows.map(|r| serde_json::from_str(&r.map_err(db_error)?).map_err(db_error))
            .collect()
    }
    pub fn connection(&self, id: &str) -> AppResult<Connection> {
        let json: Option<String> = self
            .lock()?
            .query_row("SELECT data FROM connections WHERE id=?", [id], |r| {
                r.get(0)
            })
            .optional()
            .map_err(db_error)?;
        serde_json::from_str(
            &json.ok_or_else(|| {
                AppError::new("E_NOT_FOUND", "connection", "Connection not found")
            })?,
        )
        .map_err(db_error)
    }
    pub fn save_connection(&self, value: &Connection) -> AppResult<()> {
        value.validate()?;
        self.lock()?.execute("INSERT INTO connections(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data", params![value.id,serde_json::to_string(value).map_err(db_error)?]).map_err(db_error)?;
        Ok(())
    }
    pub fn delete_connection(&self, id: &str) -> AppResult<()> {
        let conn = self.lock()?;
        let count: u32 = conn
            .query_row(
                "SELECT count(*) FROM projects WHERE connection_id=?",
                [id],
                |r| r.get(0),
            )
            .map_err(db_error)?;
        if count > 0 {
            return Err(AppError::new(
                "E_PROJECTS_EXIST",
                "connection",
                "Connection still has project indexes",
            ));
        }
        conn.execute("DELETE FROM connections WHERE id=?", [id])
            .map_err(db_error)?;
        Ok(())
    }
    pub fn projects(&self) -> AppResult<Vec<Project>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare("SELECT data FROM projects ORDER BY id")
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db_error)?;
        rows.map(|r| serde_json::from_str(&r.map_err(db_error)?).map_err(db_error))
            .collect()
    }
    pub fn project(&self, id: &str) -> AppResult<Project> {
        let json: Option<String> = self
            .lock()?
            .query_row("SELECT data FROM projects WHERE id=?", [id], |r| r.get(0))
            .optional()
            .map_err(db_error)?;
        serde_json::from_str(
            &json.ok_or_else(|| AppError::new("E_NOT_FOUND", "project", "Project not found"))?,
        )
        .map_err(db_error)
    }
    pub fn save_project(&self, project: &Project) -> AppResult<()> {
        if project.id.is_empty()
            || project.name.trim().is_empty()
            || !project.path.starts_with('/')
            || !["claude", "codex"].contains(&project.default_agent.as_str())
        {
            return Err(AppError::new(
                "E_INPUT",
                "project",
                "Invalid project settings",
            ));
        }
        let conn = self.lock()?;
        let existing: Option<String> = conn
            .query_row(
                "SELECT connection_id FROM projects WHERE id=?",
                [&project.id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if existing.is_some_and(|id| id != project.connection_id) {
            return Err(AppError::new(
                "E_TARGET",
                "project",
                "Project connection cannot be changed",
            ));
        }
        conn.execute("INSERT INTO projects(id,connection_id,data) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET data=excluded.data", params![project.id,project.connection_id,serde_json::to_string(project).map_err(db_error)?]).map_err(db_error)?;
        Ok(())
    }
    pub fn validate_target(&self, target: &Target) -> AppResult<()> {
        if target.session_id.is_some() {
            return Err(AppError::new(
                "E_CAPABILITY",
                "target",
                "Agent sessions are not available in this milestone",
            ));
        }
        self.connection(&target.connection_id)?;
        if let Some(id) = &target.project_id {
            if self.project(id)?.connection_id != target.connection_id {
                return Err(AppError::new(
                    "E_TARGET",
                    "target",
                    "Project does not belong to this connection",
                ));
            }
        }
        Ok(())
    }
    pub fn recent(&self, id: &str, user: &str) -> AppResult<Vec<String>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT path FROM recent_dirs WHERE connection_id=?1 AND remote_user=?2 ORDER BY used_at DESC LIMIT 8").map_err(db_error)?;
        let rows = stmt
            .query_map(params![id, user], |r| r.get(0))
            .map_err(db_error)?;
        rows.map(|r| r.map_err(db_error)).collect()
    }
    pub fn record_dir(&self, id: &str, user: &str, path: &str) -> AppResult<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction().map_err(db_error)?;
        tx.execute("INSERT INTO recent_dirs VALUES(?1,?2,?3,?4) ON CONFLICT(connection_id,remote_user,path) DO UPDATE SET used_at=excluded.used_at", params![id,user,path,now() as i64]).map_err(db_error)?;
        tx.execute("DELETE FROM recent_dirs WHERE connection_id=?1 AND remote_user=?2 AND path NOT IN (SELECT path FROM recent_dirs WHERE connection_id=?1 AND remote_user=?2 ORDER BY used_at DESC LIMIT 8)", params![id,user]).map_err(db_error)?;
        tx.commit().map_err(db_error)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("test.sqlite")).unwrap();
        (dir, db)
    }
    fn connection(id: &str) -> Connection {
        Connection {
            id: id.into(),
            name: id.into(),
            host: "localhost".into(),
            port: 22,
            user: "test".into(),
            auth_method: "password".into(),
            key_path: None,
            agent_kind: None,
            proxy_jump: None,
            source: None,
            last_used_at: None,
        }
    }
    #[test]
    fn stores_only_connection_metadata() {
        let (_dir, db) = fixture();
        db.save_connection(&connection("one")).unwrap();
        assert_eq!(db.connections().unwrap().len(), 1);
        assert!(!serde_json::to_string(&db.connections().unwrap())
            .unwrap()
            .contains("credential"));
    }
    #[test]
    fn recent_paths_are_bounded_and_user_isolated() {
        let (_dir, db) = fixture();
        db.save_connection(&connection("one")).unwrap();
        for n in 0..12 {
            db.record_dir("one", "u", &format!("/dir{n}")).unwrap();
        }
        assert_eq!(db.recent("one", "u").unwrap().len(), 8);
        assert!(db.recent("one", "other").unwrap().is_empty());
    }
    #[test]
    fn target_rejects_another_connections_project() {
        let (_dir, db) = fixture();
        for id in ["a", "b"] {
            db.save_connection(&connection(id)).unwrap();
        }
        let p = Project {
            id: "p".into(),
            connection_id: "a".into(),
            name: "P".into(),
            path: "/project".into(),
            description: String::new(),
            default_agent: "claude".into(),
            default_model: String::new(),
            permission_mode: "default".into(),
            pinned: false,
            archived: false,
            last_opened_at: None,
        };
        db.save_project(&p).unwrap();
        let t = Target {
            connection_id: "b".into(),
            project_id: Some("p".into()),
            session_id: None,
            request_seq: 1,
        };
        assert_eq!(db.validate_target(&t).unwrap_err().code, "E_TARGET");
        assert!(db.delete_connection("a").is_err());
    }
}
