use std::ffi::{c_char, CString};

// 日志等级常量
pub const LOG_DEBUG: u32 = 0;
pub const LOG_INFO:  u32 = 1;
pub const LOG_WARN:  u32 = 2;
pub const LOG_ERROR: u32 = 3;

/// 传递给插件的上下文
#[repr(C)]
pub struct PluginContext {
    pub api_version: u32,
    pub plugin_name: *const c_char,

    // (name, level, msg)
    pub log_fn: extern "C" fn(*const c_char, u32, *const c_char),

    // 🔥 修改: 增加第一个参数 name_ptr，用于识别发送事件的插件
    // (name, event, payload)
    pub send_event_fn: extern "C" fn(*const c_char, *const c_char, *const c_char),
}

/// 规定插件必须导出的初始化函数名称和签名
pub type InitPluginFn = unsafe extern "C" fn(*const PluginContext) -> u32;

impl PluginContext {
    /// 基础日志方法
    pub fn log(&self, level: u32, msg: &str) {
        if let Ok(c_msg) = CString::new(msg) {
            (self.log_fn)(self.plugin_name, level, c_msg.as_ptr());
        }
    }

    pub fn info(&self, msg: &str) { self.log(LOG_INFO, msg); }
    pub fn warn(&self, msg: &str) { self.log(LOG_WARN, msg); }
    pub fn error(&self, msg: &str) { self.log(LOG_ERROR, msg); }
    pub fn debug(&self, msg: &str) { self.log(LOG_DEBUG, msg); }

    /// 发送事件给前端
    pub fn emit(&self, event: &str, payload: &str) {
        let c_event = CString::new(event).unwrap_or_default();
        let c_payload = CString::new(payload).unwrap_or_default();

        // 🔥 修改: 将自己的 plugin_name 传回去，方便宿主 Debug
        (self.send_event_fn)(self.plugin_name, c_event.as_ptr(), c_payload.as_ptr());
    }
}