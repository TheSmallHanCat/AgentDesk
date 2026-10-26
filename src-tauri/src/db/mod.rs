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
        if version > 4 {
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
        if version < 2 {
            let tx = conn.transaction().map_err(db_error)?;
            tx.execute_batch("CREATE TABLE host_identities(connection_id TEXT PRIMARY KEY REFERENCES connections(id) ON DELETE CASCADE, host_id TEXT NOT NULL);
                CREATE TABLE desktop_identity(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                PRAGMA user_version=2;").map_err(db_error)?;
            tx.commit().map_err(db_error)?;
        }
        if version < 3 {
            let tx = conn.transaction().map_err(db_error)?;
            tx.execute_batch("CREATE TABLE host_sessions(session_id TEXT NOT NULL, project_id TEXT NOT NULL REFERENCES projects(id), connection_id TEXT NOT NULL REFERENCES connections(id), PRIMARY KEY(connection_id,session_id)); PRAGMA user_version=3;").map_err(db_error)?;
            tx.commit().map_err(db_error)?;
        }
        if version < 4 {
            let tx = conn.transaction().map_err(db_error)?;
            tx.execute_batch("CREATE TABLE drafts(connection_id TEXT NOT NULL, session_id TEXT NOT NULL, text TEXT NOT NULL, PRIMARY KEY(connection_id,session_id)); PRAGMA user_version=4;").map_err(db_error)?;
            tx.commit().map_err(db_error)?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
    fn lock(&self) -> AppResult<MutexGuard<'_, rusqlite::Connection>> {
        self.conn.lock().map_err(db_error)
    }
    pub fn device_id(&self) -> AppResult<String> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT OR IGNORE INTO desktop_identity(key,value) VALUES('deviceId',?1)",
            [uuid::Uuid::new_v4().to_string()],
        )
        .map_err(db_error)?;
        conn.query_row(
            "SELECT value FROM desktop_identity WHERE key='deviceId'",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)
    }
    pub fn draft(&self, target: &Target) -> AppResult<String> {
        self.validate_target(target)?;
        let session = target
            .session_id
            .as_deref()
            .ok_or_else(|| AppError::new("E_TARGET", "draft", "Session target required"))?;
        self.lock()?
            .query_row(
                "SELECT text FROM drafts WHERE connection_id=?1 AND session_id=?2",
                params![target.connection_id, session],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)
            .map(|value| value.unwrap_or_default())
    }
    pub fn save_draft(&self, target: &Target, text: &str) -> AppResult<()> {
        self.validate_target(target)?;
        let session = target
            .session_id
            .as_deref()
            .ok_or_else(|| AppError::new("E_TARGET", "draft", "Session target required"))?;
        if text.len() > 2 * 1024 * 1024 {
            return Err(AppError::new("E_LIMIT", "draft", "Draft exceeds 2 MiB"));
        }
        self.lock()?.execute("INSERT INTO drafts(connection_id,session_id,text) VALUES(?1,?2,?3) ON CONFLICT(connection_id,session_id) DO UPDATE SET text=excluded.text",params![target.connection_id,session,text]).map_err(db_error)?;
        Ok(())
    }
    pub fn forget_session(&self, target: &Target) -> AppResult<()> {
        self.validate_target(target)?;
        let conn = self.lock()?;
        conn.execute(
            "DELETE FROM drafts WHERE connection_id=?1 AND session_id=?2",
            params![target.connection_id, target.session_id],
        )
        .map_err(db_error)?;
        conn.execute(
            "DELETE FROM host_sessions WHERE connection_id=?1 AND session_id=?2",
            params![target.connection_id, target.session_id],
        )
        .map_err(db_error)?;
        Ok(())
    }
    pub fn record_host(&self, connection_id: &str, host_id: &str) -> AppResult<()> {
        let conn = self.lock()?;
        let old: Option<String> = conn
            .query_row(
                "SELECT host_id FROM host_identities WHERE connection_id=?1",
                [connection_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if old.is_some_and(|value| value != host_id) {
            return Err(AppError::new(
                "E_HOST_ID_CHANGED",
                "host.identity",
                "Host identity changed; verify the installation before reconnecting",
            ));
        }
        conn.execute(
            "INSERT OR IGNORE INTO host_identities(connection_id,host_id) VALUES(?1,?2)",
            params![connection_id, host_id],
        )
        .map_err(db_error)?;
        Ok(())
    }
    pub fn same_host_connections(&self, connection_id: &str) -> AppResult<Vec<String>> {
        let conn = self.lock()?;
        let mut statement = conn.prepare("SELECT connection_id FROM host_identities WHERE host_id=(SELECT host_id FROM host_identities WHERE connection_id=?1) AND connection_id<>?1 ORDER BY connection_id").map_err(db_error)?;
        let rows = statement
            .query_map([connection_id], |row| row.get(0))
            .map_err(db_error)?;
        rows.map(|row| row.map_err(db_error)).collect()
    }
    pub fn register_session(&self, target: &Target, session: &serde_json::Value) -> AppResult<()> {
        let id = session["id"]
            .as_str()
            .ok_or_else(|| AppError::new("E_PROTOCOL", "session.register", "Session id missing"))?;
        let project = session["projectId"].as_str().ok_or_else(|| {
            AppError::new("E_PROTOCOL", "session.register", "Session project missing")
        })?;
        if target.project_id.as_deref() != Some(project) {
            return Err(AppError::new(
                "E_TARGET",
                "session.register",
                "Session belongs to another project",
            ));
        }
        self.validate_target(&Target {
            session_id: None,
            ..target.clone()
        })?;
        self.lock()?.execute("INSERT OR IGNORE INTO host_sessions(session_id,project_id,connection_id) VALUES(?1,?2,?3)", params![id,project,target.connection_id]).map_err(db_error)?;
        self.validate_target(&Target {
            session_id: Some(id.into()),
            ..target.clone()
        })
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
            || project.default_agent.is_empty()
            || project.default_agent.len() > 64
            || !project
                .default_agent
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
            || !["default", "plan"].contains(&project.permission_mode.as_str())
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
        if let Some(session) = &target.session_id {
            let owner: Option<(String, String)> = self.lock()?.query_row("SELECT connection_id,project_id FROM host_sessions WHERE session_id=?1 AND connection_id=?2", params![session,target.connection_id], |row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(db_error)?;
            if !owner.is_some_and(|(connection, project)| {
                connection == target.connection_id
                    && Some(project.as_str()) == target.project_id.as_deref()
            }) {
                return Err(AppError::new(
                    "E_TARGET",
                    "target",
                    "Session does not belong to this project and connection",
                ));
            }
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
        let last:i64=tx.query_row("SELECT COALESCE(MAX(used_at),0) FROM recent_dirs WHERE connection_id=?1 AND remote_user=?2",params![id,user],|row|row.get(0)).map_err(db_error)?;
        let used_at = (now() as i64).max(last.saturating_add(1));
        tx.execute("INSERT INTO recent_dirs VALUES(?1,?2,?3,?4) ON CONFLICT(connection_id,remote_user,path) DO UPDATE SET used_at=excluded.used_at", params![id,user,path,used_at]).map_err(db_error)?;
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
        assert_eq!(db.recent("one", "u").unwrap()[0], "/dir11");
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
