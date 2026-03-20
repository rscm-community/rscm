use crate::config::SourceConfig;
use mlua::{Lua, Table, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub struct BuiltinState {
    pub sources: Mutex<HashMap<String, SourceConfig>>,
    pub source_data: Mutex<HashMap<String, Table>>,
}

impl Default for BuiltinState {
    fn default() -> Self {
        Self {
            sources: Mutex::new(HashMap::new()),
            source_data: Mutex::new(HashMap::new()),
        }
    }
}

pub fn register_builtins(lua: &Lua, state: Arc<BuiltinState>) -> mlua::Result<()> {
    let globals = lua.globals();

    let sources_fn = lua.create_function(move |_lua: &Lua, params: Table| {
        let mut sources_map = state.sources.lock().unwrap();
        for pair in params.pairs::<String, Table>() {
            let (name, source_table) = pair?;
            let mut config = SourceConfig::default();

            if let Ok(source_type) = source_table.get::<String>("type") {
                config.source_type = match source_type.as_str() {
                    "github" => crate::config::SourceType::GitHub,
                    "path" => crate::config::SourceType::Path,
                    "directurl" => crate::config::SourceType::DirectUrl,
                    _ => crate::config::SourceType::Path,
                };
            }
            config.owner = source_table.get::<String>("owner").ok();
            config.repo = source_table.get::<String>("repo").ok();
            config.r#ref = source_table.get::<String>("ref").ok();
            config.path = source_table.get::<String>("path").ok();
            config.track_git = source_table.get::<bool>("track_git").ok();
            config.url = source_table.get::<String>("url").ok();
            config.hash = source_table.get::<String>("hash").ok();

            sources_map.insert(name, config);
        }
        Ok(())
    })?;
    globals.set("sources", sources_fn)?;

    let extend_fn = lua.create_function(
        |lua: &Lua, args: mlua::Variadic<mlua::Value>| -> mlua::Result<Table> {
            if args.is_empty() || args.len() > 2 {
                return Err(mlua::Error::RuntimeError(
                    "extend expects 1 or 2 arguments".to_string(),
                ));
            }

            let result = lua.create_table()?;

            if let Some(name) = args[0].as_string() {
                let name_str = name.to_str()?.to_string();
                result.set("inherits", vec![name_str])?;
            } else if let Some(base) = args[0].as_table() {
                if let Ok(base_inherits) = base.get::<Vec<String>>("inherits") {
                    result.set("inherits", base_inherits)?;
                }
                for pair in base.pairs::<Value, Value>() {
                    let (k, v) = pair?;
                    let is_inherits_key = k
                        .as_string()
                        .and_then(|s| s.to_str().ok())
                        .map_or(false, |s| s == "inherits");
                    if is_inherits_key {
                        continue;
                    }
                    if let Some(v_table) = v.as_table() {
                        let new_table = lua.create_table()?;
                        deep_copy_table_to(lua, &v_table, &new_table)?;
                        result.set(k, new_table)?;
                    } else {
                        result.set(k, v)?;
                    }
                }
            } else {
                return Err(mlua::Error::RuntimeError(
                    "expected string or table as first argument".to_string(),
                ));
            }

            if args.len() == 2 {
                if let Some(override_tbl) = args[1].as_table() {
                    deep_merge_to(lua, &result, &override_tbl)?;
                } else {
                    return Err(mlua::Error::RuntimeError(
                        "expected table as second argument".to_string(),
                    ));
                }
            }

            Ok(result)
        },
    )?;
    globals.set("extend", extend_fn)?;

    let merge_fn = lua.create_function(|lua: &Lua, tables: Table| {
        let result = lua.create_table()?;
        for item in tables.sequence_values::<Table>() {
            let table = item?;
            deep_merge_to(lua, &result, &table)?;
        }
        Ok(result)
    })?;
    globals.set("merge", merge_fn)?;

    let github_fn = lua.create_function(
        |_lua: &Lua, (owner, repo, ref_opt): (String, String, Option<String>)| {
            let table = _lua.create_table()?;
            table.set("type", "github")?;
            table.set("owner", owner)?;
            table.set("repo", repo)?;
            table.set("ref", ref_opt.unwrap_or_else(|| "main".to_string()))?;
            Ok(table)
        },
    )?;
    globals.set("github", github_fn)?;

    let github_block_fn = lua.create_function(|lua: &Lua, params: Table| {
        let owner: String = params.get("owner")?;
        let repo: String = params.get("repo")?;
        let ref_opt: Option<String> = params.get("ref").ok();

        let table = lua.create_table()?;
        table.set("type", "github")?;
        table.set("owner", owner)?;
        table.set("repo", repo)?;
        if let Some(r) = ref_opt {
            table.set("ref", r)?;
        }
        Ok(table)
    })?;
    globals.set("github", github_block_fn)?;

    let path_fn = lua.create_function(|_lua: &Lua, path_str: String| {
        let table = _lua.create_table()?;
        table.set("type", "path")?;
        table.set("path", path_str)?;
        Ok(table)
    })?;
    globals.set("path", path_fn)?;

    let path_block_fn = lua.create_function(|lua: &Lua, params: Table| {
        let path: String = params.get("path")?;
        let track_git: Option<bool> = params.get("track_git").ok();

        let table = lua.create_table()?;
        table.set("type", "path")?;
        table.set("path", path)?;
        if let Some(tg) = track_git {
            table.set("track_git", tg)?;
        }
        Ok(table)
    })?;
    globals.set("path", path_block_fn)?;

    let source_fn =
        lua.create_function(move |_lua: &Lua, name: String| -> mlua::Result<Table> {
            let table = _lua.create_table()?;
            table.set("__source_name", name.clone())?;

            let metatable = _lua.create_table()?;
            let index_fn = _lua.create_function(
                move |_lua2: &Lua, (src_table, key): (Table, String)| -> mlua::Result<Table> {
                    let current_name: String = src_table
                        .get("__source_name")
                        .unwrap_or_else(|_| name.clone());
                    let result = _lua2.create_table()?;
                    result.set("__source_name", format!("{}.{}", current_name, key))?;
                    Ok(result)
                },
            )?;
            metatable.set("__index", index_fn)?;
            let _ = table.set_metatable(Some(metatable));

            Ok(table)
        })?;
    globals.set("source", source_fn)?;

    Ok(())
}

fn deep_copy_table_to(lua: &Lua, src: &Table, dest: &Table) -> mlua::Result<()> {
    for pair in src.pairs::<Value, Value>() {
        let (k, v) = pair?;
        if let Some(src_table) = v.as_table() {
            let new_table = lua.create_table()?;
            deep_copy_table_to(lua, &src_table, &new_table)?;
            dest.set(k, new_table)?;
        } else {
            dest.set(k, v)?;
        }
    }
    Ok(())
}

fn deep_merge_to(lua: &Lua, base: &Table, override_: &Table) -> mlua::Result<()> {
    let inherits_key = Value::String(lua.create_string("inherits")?);
    for pair in override_.pairs::<Value, Value>() {
        let (k, v) = pair?;
        if k == inherits_key {
            if let Ok(base_inherits) = base.get::<Vec<String>>("inherits") {
                if let Some(override_inherits) = v.as_table() {
                    let mut merged_inherits = base_inherits;
                    for item in override_inherits.sequence_values::<String>() {
                        if let Ok(inherit) = item {
                            if !merged_inherits.contains(&inherit) {
                                merged_inherits.push(inherit);
                            }
                        }
                    }
                    base.set(k, merged_inherits)?;
                    continue;
                }
            }
            continue;
        }
        if let Some(v_table) = v.as_table() {
            if let Ok(existing) = base.get::<Table>(k.clone()) {
                deep_merge_to(lua, &existing, &v_table)?;
            } else {
                let new_table = lua.create_table()?;
                deep_copy_table_to(lua, &v_table, &new_table)?;
                base.set(k, new_table)?;
            }
        } else {
            base.set(k, v)?;
        }
    }
    Ok(())
}
