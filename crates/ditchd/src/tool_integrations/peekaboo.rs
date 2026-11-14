use super::*;
pub struct Peekaboo;
impl ToolIntegration for Peekaboo {
    fn id(&self) -> &'static str {
        "peekaboo"
    }
    fn tools(&self) -> Vec<String> {
        desktop_tool_schemas().into_iter().map(|t| t.0).collect()
    }
    fn invoke(
        &self,
        request: &Value,
        cancelled: &Arc<AtomicBool>,
        lock: Option<std::fs::File>,
    ) -> Value {
        native::invoke(request, cancelled, lock)
    }
}
