use std::path::{Path, PathBuf};

use schemars::schema_for;
use serde::Serialize;

use crate::{resolve_agent_trail_dir, AppConfig, AppError};

use super::types::{ControlFrame, OutFrame, ResponseFrame, ServeCommand};

#[derive(Serialize)]
pub(crate) struct ServeSchemaBundle {
    serve_command: schemars::schema::RootSchema,
    control_frame: schemars::schema::RootSchema,
    response_frame: schemars::schema::RootSchema,
    out_frame: schemars::schema::RootSchema,
}

pub fn schema_output_dir(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    if let Some(path) = cfg.serve.schema_out_dir.as_deref() {
        return crate::normalize_path(path);
    }
    Ok(resolve_agent_trail_dir(cfg)?.join("serve-schema"))
}

pub(crate) fn build_schema_bundle() -> ServeSchemaBundle {
    ServeSchemaBundle {
        serve_command: schema_for!(ServeCommand),
        control_frame: schema_for!(ControlFrame),
        response_frame: schema_for!(ResponseFrame),
        out_frame: schema_for!(OutFrame),
    }
}

pub fn write_schema_bundle(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    let out_dir = schema_output_dir(cfg)?;
    std::fs::create_dir_all(&out_dir).map_err(AppError::Io)?;
    let schema_path = out_dir.join("serve.schema.json");
    let dts_path = out_dir.join("serve.d.ts");
    std::fs::write(
        &schema_path,
        serde_json::to_vec_pretty(&build_schema_bundle())
            .map_err(|error| AppError::Config(format!("serialize serve schema failed: {error}")))?,
    )
    .map_err(AppError::Io)?;
    std::fs::write(&dts_path, serve_dts()).map_err(AppError::Io)?;
    Ok(out_dir)
}

pub fn serve_dts() -> &'static str {
    r#"export type ServeCommand = unknown;
export type ControlFrame = unknown;
export type ResponseFrame = unknown;
export type OutFrame = unknown;
"#
}

pub fn read_schema_fixture(path: &Path) -> Result<String, AppError> {
    std::fs::read_to_string(path).map_err(AppError::Io)
}
