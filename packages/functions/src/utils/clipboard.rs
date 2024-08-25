use log::error;

pub fn copy_to_clipboard(val: impl ToString) {
    let val = val.to_string().replace('"', r#"\""#);
    let js_code = format!(r#"navigator.clipboard.writeText("{}");"#, val);
    match js_sys::eval(js_code.as_str()) {
        Ok(_) => {}
        Err(e) => {
            error!("{:?}", e);
        }
    }
}
