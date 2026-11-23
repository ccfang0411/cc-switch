use super::Database;
use crate::error::AppError;
use rusqlite::{params, Connection};
use serde_json::Value;

pub(super) fn migrate(conn: &Connection) -> Result<(), AppError> {
    for table in ["mcp_servers", "skills"] {
        if !Database::table_exists(conn, table)? {
            continue;
        }
        for column in ["enabled_kimi", "enabled_agy"] {
            Database::add_column_if_missing(conn, table, column, "BOOLEAN NOT NULL DEFAULT 0")?;
        }
        if Database::has_column(conn, table, "enabled_kimicode")? {
            conn.execute(&format!("UPDATE {table} SET enabled_kimi = CASE WHEN enabled_kimi OR enabled_kimicode THEN 1 ELSE 0 END"), [])?;
        }
    }
    extend_proxy_check(conn)?;
    migrate_app_rows(conn)?;
    normalize_providers(conn)?;
    migrate_profile_slots(conn)?;
    if Database::table_exists(conn, "proxy_config")? {
        conn.execute(
            "INSERT OR IGNORE INTO proxy_config (app_type) VALUES ('kimi')",
            [],
        )?;
    }
    let invalid: bool = conn.prepare("PRAGMA foreign_key_check")?.exists([])?;
    if invalid {
        return Err(AppError::Database(
            "Kimi compatibility migration found invalid foreign keys".into(),
        ));
    }
    Ok(())
}

fn column_names(conn: &Connection, table: &str) -> Result<Vec<String>, AppError> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({})", quoted(table)))?;
    let names = statement
        .query_map([], |row| row.get(1))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(names)
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn extend_proxy_check(conn: &Connection) -> Result<(), AppError> {
    if !Database::table_exists(conn, "proxy_config")? {
        return Ok(());
    }
    let sql: String = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='proxy_config'",
        [],
        |row| row.get(0),
    )?;
    let check = regex::Regex::new(r#"(?i)CHECK\s*\(\s*"?app_type"?\s+IN\s*\(([^)]*)\)\s*\)"#)
        .expect("constant regex");
    let Some(capture) = check.captures(&sql) else {
        if sql.to_ascii_lowercase().contains("check") {
            return Err(AppError::Database(
                "Unrecognized proxy_config constraint; refusing a lossy migration".into(),
            ));
        }
        return Ok(());
    };
    if capture[1].split(',').any(|value| value.trim() == "'kimi'") {
        return Ok(());
    }
    let header = regex::Regex::new(r#"(?i)^CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?(?:"proxy_config"|`proxy_config`|\[proxy_config\]|proxy_config)\s*\("#).expect("constant regex");
    if !header.is_match(&sql) {
        return Err(AppError::Database(
            "Unrecognized proxy_config definition; refusing a lossy migration".into(),
        ));
    }
    let allowed = format!("CHECK (app_type IN ({}, 'kimi', 'kimicode'))", &capture[1]);
    let sql = check.replace(&sql, allowed.as_str());
    let sql = header.replace(&sql, "CREATE TABLE _ccs_proxy_v28 (");
    let objects = {
        let mut statement = conn.prepare("SELECT sql FROM sqlite_master WHERE tbl_name='proxy_config' AND type IN ('index','trigger') AND sql IS NOT NULL ORDER BY type, name")?;
        let objects = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        objects
    };
    let columns = column_names(conn, "proxy_config")?
        .iter()
        .map(|name| quoted(name))
        .collect::<Vec<_>>()
        .join(",");
    conn.execute_batch(&sql)?;
    conn.execute(
        &format!("INSERT INTO _ccs_proxy_v28 ({columns}) SELECT {columns} FROM proxy_config"),
        [],
    )?;
    conn.execute("DROP TABLE proxy_config", [])?;
    conn.execute("ALTER TABLE _ccs_proxy_v28 RENAME TO proxy_config", [])?;
    for object in objects {
        conn.execute_batch(&object)?;
    }
    Ok(())
}

fn migrate_app_rows(conn: &Connection) -> Result<(), AppError> {
    let tables = {
        let mut statement = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
        let tables = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        tables
    };
    if Database::table_exists(conn, "providers")? {
        let columns = column_names(conn, "providers")?;
        let select = columns
            .iter()
            .map(|name| {
                if name == "app_type" {
                    "'kimi'".into()
                } else {
                    quoted(name)
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        let columns = columns
            .iter()
            .map(|name| quoted(name))
            .collect::<Vec<_>>()
            .join(",");
        conn.execute(&format!("INSERT INTO providers ({columns}) SELECT {select} FROM providers WHERE app_type='kimicode'"), [])?;
    }
    for table in tables {
        if table == "providers"
            || !column_names(conn, &table)?
                .iter()
                .any(|column| column == "app_type")
        {
            continue;
        }
        conn.execute(
            &format!(
                "UPDATE {} SET app_type='kimi' WHERE app_type='kimicode'",
                quoted(&table)
            ),
            [],
        )?;
    }
    if Database::table_exists(conn, "providers")? {
        conn.execute("DELETE FROM providers WHERE app_type='kimicode'", [])?;
    }
    if Database::table_exists(conn, "settings")? {
        for prefix in [
            "current_provider_",
            "current_profile_id_",
            "common_config_",
            "proxy_takeover_",
            "auto_failover_enabled_",
        ] {
            let old = format!("{prefix}kimicode");
            let new = format!("{prefix}kimi");
            conn.execute("UPDATE settings SET key=?1 WHERE key=?2", params![new, old])?;
        }
    }
    Ok(())
}

fn normalize_providers(conn: &Connection) -> Result<(), AppError> {
    if !Database::table_exists(conn, "providers")?
        || !Database::has_column(conn, "providers", "settings_config")?
    {
        return Ok(());
    }
    let rows = {
        let mut statement =
            conn.prepare("SELECT id, settings_config FROM providers WHERE app_type='kimi'")?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for (id, text) in rows {
        let config: Value = serde_json::from_str(&text).map_err(|_| {
            AppError::Database(format!("Invalid Kimi provider configuration: {id}"))
        })?;
        let canonical = crate::kimi_config::canonical_config(&id, &config)?;
        if config != canonical {
            conn.execute(
                "UPDATE providers SET settings_config=?1 WHERE app_type='kimi' AND id=?2",
                params![
                    serde_json::to_string(&canonical)
                        .map_err(|error| AppError::Database(error.to_string()))?,
                    id
                ],
            )?;
        }
    }
    Ok(())
}

fn migrate_profile_slots(conn: &Connection) -> Result<(), AppError> {
    if !Database::table_exists(conn, "profiles")?
        || !Database::has_column(conn, "profiles", "payload")?
    {
        return Ok(());
    }
    let rows = {
        let mut statement = conn.prepare("SELECT id, payload FROM profiles")?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for (id, text) in rows {
        let mut payload: Value = serde_json::from_str(&text)
            .map_err(|_| AppError::Database(format!("Invalid profile payload: {id}")))?;
        let mut changed = false;
        for field in ["providers", "mcp", "skills", "prompts"] {
            let Some(slots) = payload.get_mut(field).and_then(Value::as_object_mut) else {
                continue;
            };
            let Some(legacy) = slots.remove("kimicode") else {
                continue;
            };
            if slots
                .get("kimi")
                .is_some_and(|current| !current.is_null() && current != &legacy)
            {
                return Err(AppError::Database(format!(
                    "Profile {id} contains conflicting Kimi {field} slots"
                )));
            }
            slots.insert("kimi".into(), legacy);
            changed = true;
        }
        if !changed {
            continue;
        }
        conn.execute(
            "UPDATE profiles SET payload=?1 WHERE id=?2",
            params![
                serde_json::to_string(&payload)
                    .map_err(|error| AppError::Database(error.to_string()))?,
                id
            ],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_fork_migration_preserves_flags_relations_and_custom_proxy_columns(
    ) -> Result<(), AppError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;
            CREATE TABLE providers(id TEXT, app_type TEXT, settings_config TEXT, PRIMARY KEY(id,app_type));
            CREATE TABLE provider_endpoints(id INTEGER PRIMARY KEY,provider_id TEXT,app_type TEXT,url TEXT,FOREIGN KEY(provider_id,app_type) REFERENCES providers(id,app_type) ON DELETE CASCADE);
            CREATE TABLE mcp_servers(id TEXT PRIMARY KEY,enabled_kimicode BOOLEAN,enabled_agy BOOLEAN);
            CREATE TABLE skills(id TEXT PRIMARY KEY,enabled_kimicode BOOLEAN,enabled_kimi BOOLEAN);
            CREATE TABLE proxy_config(app_type TEXT PRIMARY KEY CHECK(app_type IN ('claude','codex','gemini','grokbuild','kimicode')),listen_port INTEGER DEFAULT 15721,max_retries INTEGER DEFAULT 3,default_cost_multiplier TEXT DEFAULT '1',future_option TEXT DEFAULT 'keep');
            CREATE INDEX custom_proxy_index ON proxy_config(future_option);
            INSERT INTO mcp_servers VALUES('m',1,1);
            INSERT INTO skills VALUES('s',1,0);
            INSERT INTO proxy_config VALUES('kimicode',23456,7,'1.7','custom');")?;
        let config = json!({"type":"openai", "base_url":"https://example.test/v1", "model":{"model":"deepseek", "max_context_size":1000}, "future":true});
        conn.execute(
            "INSERT INTO providers VALUES('vendor','kimicode',?1)",
            [config.to_string()],
        )?;
        conn.execute(
            "INSERT INTO provider_endpoints VALUES(1,'vendor','kimicode','https://example.test')",
            [],
        )?;
        Database::in_savepoint(&conn, "compat_test", || migrate(&conn))?;
        let values: (i64, i64, String, String) = conn.query_row("SELECT listen_port,max_retries,default_cost_multiplier,future_option FROM proxy_config WHERE app_type='kimi'", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))?;
        assert_eq!(values, (23456, 7, "1.7".into(), "custom".into()));
        assert_eq!(
            conn.query_row("SELECT app_type FROM provider_endpoints", [], |row| row
                .get::<_, String>(
                0
            ))?,
            "kimi"
        );
        for table in ["mcp_servers", "skills"] {
            assert_eq!(
                conn.query_row(&format!("SELECT enabled_kimi FROM {table}"), [], |row| row
                    .get::<_, bool>(
                    0
                ))?,
                true
            );
            assert!(Database::has_column(&conn, table, "enabled_agy")?);
        }
        let saved: String = conn.query_row("SELECT settings_config FROM providers", [], |row| {
            row.get(0)
        })?;
        assert_eq!(
            serde_json::from_str::<Value>(&saved).unwrap()["future"],
            true
        );
        assert!(!conn.prepare("PRAGMA foreign_key_check")?.exists([])?);
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='custom_proxy_index'",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            1
        );
        Database::in_savepoint(&conn, "compat_again", || migrate(&conn))?;
        Ok(())
    }

    #[test]
    fn legacy_v23_upgrade_preserves_private_and_upstream_data() -> Result<(), AppError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;
            CREATE TABLE providers(id TEXT, app_type TEXT, settings_config TEXT, meta TEXT, PRIMARY KEY(id,app_type));
            CREATE TABLE provider_endpoints(id INTEGER PRIMARY KEY,provider_id TEXT,app_type TEXT,url TEXT,FOREIGN KEY(provider_id,app_type) REFERENCES providers(id,app_type) ON DELETE CASCADE);
            CREATE TABLE mcp_servers(id TEXT PRIMARY KEY,enabled_kimicode BOOLEAN);
            CREATE TABLE skills(id TEXT PRIMARY KEY,enabled_kimicode BOOLEAN);
            CREATE TABLE skill_repos(owner TEXT, name TEXT);
            CREATE TABLE session_log_sync(file_path TEXT PRIMARY KEY,last_modified INTEGER,last_line_offset INTEGER,last_synced_at INTEGER);
            CREATE TABLE prompts(id TEXT,app_type TEXT,content TEXT,PRIMARY KEY(id,app_type));
            CREATE TABLE proxy_config(app_type TEXT PRIMARY KEY CHECK(app_type IN ('claude','kimicode','future-app')),listen_port INTEGER DEFAULT 15721,future_option TEXT DEFAULT 'keep');
            CREATE TABLE proxy_audit(app_type TEXT);
            CREATE TRIGGER custom_proxy_insert AFTER INSERT ON proxy_config BEGIN INSERT INTO proxy_audit VALUES(NEW.app_type); END;
            CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT);
            CREATE TABLE profiles(id TEXT PRIMARY KEY,payload TEXT);
            INSERT INTO mcp_servers VALUES('mcp',1);
            INSERT INTO skills VALUES('skill',1);
            INSERT INTO skill_repos VALUES('owner','repo');
            INSERT INTO session_log_sync VALUES('kimi:history',11,123,12);
            INSERT INTO proxy_config VALUES('kimicode',23456,'custom');
            INSERT INTO proxy_config VALUES('future-app',23457,'future');
            INSERT INTO settings VALUES('current_provider_kimicode','vendor');
            INSERT INTO prompts VALUES('prompt','kimicode','custom instruction');")?;
        let config = json!({"type":"openai", "base_url":"https://example.test/v1", "api_key":"test", "model":{"model":"deepseek", "max_context_size":1000}, "future":true});
        conn.execute(
            "INSERT INTO providers VALUES('vendor','kimicode',?1,?2)",
            params![
                config.to_string(),
                "{\"authBinding\":{\"accountId\":\"account\"}}"
            ],
        )?;
        conn.execute(
            "INSERT INTO provider_endpoints VALUES(1,'vendor','kimicode','https://example.test')",
            [],
        )?;
        let payload = json!({"providers":{"kimicode":{"id":"vendor"},"claude":{"id":"other"}},"mcp":{"kimicode":["mcp"]},"skills":{"kimicode":["skill"]},"prompts":{"kimicode":"prompt"},"future":{"keep":true}});
        conn.execute(
            "INSERT INTO profiles VALUES('profile',?1)",
            [payload.to_string()],
        )?;
        Database::set_user_version(&conn, 23)?;
        Database::apply_schema_migrations_on_conn(&conn)?;
        assert_eq!(
            Database::get_user_version(&conn)?,
            super::super::SCHEMA_VERSION
        );
        for table in ["mcp_servers", "skills"] {
            assert!(
                conn.query_row(&format!("SELECT enabled_kimi FROM {table}"), [], |row| row
                    .get::<_, bool>(
                    0
                ))?
            );
            for column in [
                "enabled_agy",
                "enabled_dsh",
                "enabled_copilot",
                "enabled_workbuddy",
            ] {
                assert!(Database::has_column(&conn, table, column)?);
            }
        }
        assert!(Database::has_column(&conn, "mcp_servers", "enabled_omp")?);
        assert!(Database::has_column(
            &conn,
            "session_log_sync",
            "parser_state"
        )?);
        assert_eq!(
            conn.query_row("SELECT last_line_offset FROM session_log_sync", [], |row| {
                row.get::<_, i64>(0)
            })?,
            123
        );
        assert_eq!(
            conn.query_row("SELECT host FROM skill_repos", [], |row| row
                .get::<_, String>(0))?,
            "github.com"
        );
        assert_eq!(
            conn.query_row(
                "SELECT listen_port FROM proxy_config WHERE app_type='kimi'",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            23456
        );
        assert_eq!(
            conn.query_row(
                "SELECT future_option FROM proxy_config WHERE app_type='future-app'",
                [],
                |row| row.get::<_, String>(0)
            )?,
            "future"
        );
        conn.execute("INSERT INTO proxy_config(app_type) VALUES('claude')", [])?;
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM proxy_audit WHERE app_type='claude'",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            1
        );
        let saved: String = conn.query_row(
            "SELECT settings_config FROM providers WHERE app_type='kimi'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            serde_json::from_str::<Value>(&saved).unwrap()["future"],
            true
        );
        assert_eq!(
            conn.query_row(
                "SELECT meta FROM providers WHERE app_type='kimi'",
                [],
                |row| row.get::<_, String>(0)
            )?,
            "{\"authBinding\":{\"accountId\":\"account\"}}"
        );
        for table in ["provider_endpoints", "prompts"] {
            assert_eq!(
                conn.query_row(&format!("SELECT app_type FROM {table}"), [], |row| row
                    .get::<_, String>(
                    0
                ))?,
                "kimi"
            );
        }
        assert_eq!(
            conn.query_row(
                "SELECT value FROM settings WHERE key='current_provider_kimi'",
                [],
                |row| row.get::<_, String>(0)
            )?,
            "vendor"
        );
        let saved: String = conn.query_row("SELECT payload FROM profiles", [], |row| row.get(0))?;
        let saved: Value = serde_json::from_str(&saved).unwrap();
        for field in ["providers", "mcp", "skills", "prompts"] {
            assert_eq!(saved[field]["kimi"], payload[field]["kimicode"]);
            assert!(saved[field].get("kimicode").is_none());
        }
        assert_eq!(saved["providers"]["claude"], payload["providers"]["claude"]);
        assert_eq!(saved["future"], payload["future"]);
        assert!(!conn.prepare("PRAGMA foreign_key_check")?.exists([])?);
        Database::apply_schema_migrations_on_conn(&conn)?;
        Ok(())
    }

    #[test]
    fn legacy_v23_conflict_rolls_back_the_entire_upgrade() -> Result<(), AppError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("CREATE TABLE providers(id TEXT,app_type TEXT,PRIMARY KEY(id,app_type)); INSERT INTO providers VALUES('same','kimi'),('same','kimicode'); CREATE TABLE skills(id TEXT,enabled_kimicode BOOLEAN); INSERT INTO skills VALUES('s',1);")?;
        Database::set_user_version(&conn, 23)?;
        assert!(Database::apply_schema_migrations_on_conn(&conn).is_err());
        assert_eq!(Database::get_user_version(&conn)?, 23);
        assert!(!Database::has_column(&conn, "skills", "enabled_dsh")?);
        assert!(!Database::has_column(&conn, "skills", "enabled_kimi")?);
        assert_eq!(
            conn.query_row("SELECT count(*) FROM providers", [], |row| row
                .get::<_, i64>(0))?,
            2
        );
        Ok(())
    }

    #[test]
    fn colliding_app_ids_roll_back_instead_of_overwriting() -> Result<(), AppError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("CREATE TABLE providers(id TEXT,app_type TEXT,PRIMARY KEY(id,app_type)); INSERT INTO providers VALUES('same','kimi'),('same','kimicode'); CREATE TABLE skills(id TEXT,enabled_kimicode BOOLEAN); INSERT INTO skills VALUES('s',1);")?;
        assert!(Database::in_savepoint(&conn, "compat_conflict", || migrate(&conn)).is_err());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM providers", [], |row| row
                .get::<_, i64>(0))?,
            2
        );
        assert!(!Database::has_column(&conn, "skills", "enabled_kimi")?);
        Ok(())
    }
}
