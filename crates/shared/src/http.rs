use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ApiMessage<T>
where
    T: Serialize,
{
    pub success: bool,
    pub msg: String,
    pub obj: Option<T>,
}

impl<T> ApiMessage<T>
where
    T: Serialize,
{
    pub fn success(obj: T) -> Self {
        Self { success: true, msg: String::new(), obj: Some(obj) }
    }

    pub fn success_without_obj() -> Self {
        Self { success: true, msg: String::new(), obj: None }
    }

    pub fn action(msg: impl Into<String>) -> Self {
        Self { success: true, msg: msg.into(), obj: None }
    }

    pub fn failure(msg: impl Into<String>) -> Self {
        Self { success: false, msg: msg.into(), obj: None }
    }
}
