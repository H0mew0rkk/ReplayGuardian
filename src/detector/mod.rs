pub mod mcp;
pub mod registry;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    On,
    Off,
    Unknown,
}
impl State {
    pub fn label(self) -> &'static str {
        match self {
            Self::On => "已开启",
            Self::Off => "已关闭",
            Self::Unknown => "状态未知",
        }
    }
}
pub fn parse_status(value: &serde_json::Value) -> Result<State, String> {
    if value.get("error").is_some() {
        return Err("MCP 协议错误".into());
    }
    let result = &value["result"];
    if result["isError"] == true || result["status"] == "error" {
        return Err(format!(
            "MCP 工具错误：{}",
            result.get("content").unwrap_or(&serde_json::Value::Null)
        ));
    }
    match result["structuredContent"]["instantReplayEnabled"].as_bool() {
        Some(true) => Ok(State::On),
        Some(false) => Ok(State::Off),
        None => Err("MCP 未提供有效的 instantReplayEnabled 字段".into()),
    }
}
pub fn parse_registry(kind: u32, bytes: &[u8]) -> Result<State, String> {
    match (kind, bytes) {
        (3, [1, 0, 0, 0]) => Ok(State::On),
        (3, [0, 0, 0, 0]) => Ok(State::Off),
        _ => Err("注册表类型或状态编码未经验证".into()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn missing_is_not_off() {
        for value in [
            json!({}),
            json!({"result":{"structuredContent":{"instantReplayEnabled":null}}}),
            json!({"result":{"structuredContent":{"instantReplayEnabled":0}}}),
        ] {
            assert!(parse_status(&value).is_err());
        }
    }
    #[test]
    fn tool_error_overrides_false() {
        assert!(parse_status(&json!({"result":{"isError":true,"structuredContent":{"instantReplayEnabled":false}}})).is_err());
    }
    #[test]
    fn boolean_status_only() {
        assert_eq!(
            parse_status(&json!({"result":{"structuredContent":{"instantReplayEnabled":false}}}))
                .unwrap(),
            State::Off
        );
    }
    #[test]
    fn registry_strict_encoding() {
        assert_eq!(parse_registry(3, &[1, 0, 0, 0]).unwrap(), State::On);
        for (k, b) in [(4, vec![0, 0, 0, 0]), (3, vec![0]), (3, vec![2, 0, 0, 0])] {
            assert!(parse_registry(k, &b).is_err());
        }
    }
}
