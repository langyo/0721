use log::error;

pub fn copy_to_clipboard(val: &str) {
    let val = val.replace('"', r#"\""#);
    let js_code = format!(r#"navigator.clipboard.writeText("{}");"#, val);
    match js_sys::eval(js_code.as_str()) {
        Ok(_) => {}
        Err(e) => {
            error!("{:?}", e);
        }
    }
}
