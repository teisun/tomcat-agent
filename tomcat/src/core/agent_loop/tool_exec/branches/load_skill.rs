use super::super::args::parse_load_skill_args;
use super::super::ToolExecCtx;

pub(in super::super) async fn handle_load_skill(
    ctx: &ToolExecCtx<'_>,
    args: &serde_json::Value,
) -> Result<String, String> {
    let (name, file) = parse_load_skill_args(args)?;
    let skill_set = ctx
        .skill_set
        .ok_or_else(|| "load_skill runtime was not supplied".to_string())?;
    let (skill, visible_names, hidden_from_model) = {
        let snapshot = skill_set.read();
        (
            snapshot.resolve(name).cloned(),
            crate::core::skill::visible_skill_names_csv(&snapshot),
            snapshot.resolve(name).is_none() && snapshot.resolve_any(name).is_some(),
        )
    };

    let skill = match skill {
        Some(skill) => skill,
        None => {
            if hidden_from_model {
                return Err(format!(
                    "Skill `{name}` is user-only; the model may not invoke it through load_skill"
                ));
            }
            let available = if visible_names.is_empty() {
                "<none>".to_string()
            } else {
                visible_names
            };
            return Err(format!(
                "Unknown Skill `{name}`. Available Skills: {available}"
            ));
        }
    };

    crate::core::skill::load_skill_payload(ctx.primitive.as_ref(), "__agent__", &skill, file).await
}
