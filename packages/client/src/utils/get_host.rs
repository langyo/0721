use anyhow::{anyhow, Result};

pub fn get_host() -> Result<String> {
    let js_code = r"window.location.protocol + '//' + window.location.host";
    match js_sys::eval(js_code) {
        Ok(val) => val
            .as_string()
            .ok_or(anyhow!("Failed to get the javascript string value.")),
        Err(e) => Err(anyhow!("{:?}", e)),
    }
}
