use std::{cell::RefCell, rc::Rc};
use uuid::Uuid;
use wasm_bindgen::{closure::Closure, JsCast as _};
use web_sys::Blob;

#[derive(Clone, Debug)]
pub struct FileUploader {
    pub file_blobs: Rc<RefCell<Vec<Blob>>>,
    pub file_names: Rc<RefCell<Vec<String>>>,
    pub element: web_sys::HtmlInputElement,
}

impl FileUploader {
    pub fn new(accept: impl ToString) -> Self {
        let id = Uuid::new_v4().to_string();
        let id = format!("file-uploader-{}", id);

        let input = gloo::utils::document().create_element("input").unwrap();
        input.set_attribute("type", "file").unwrap();
        input.set_attribute("id", &id).unwrap();
        input.set_attribute("style", "display: none").unwrap();
        input.set_attribute("accept", &accept.to_string()).unwrap();
        input.set_attribute("multiple", "true").unwrap();

        gloo::utils::document()
            .body()
            .unwrap()
            .append_child(&input)
            .unwrap();

        let element = input.dyn_into::<web_sys::HtmlInputElement>().unwrap();
        let file_blobs = Rc::new(RefCell::new(vec![]));
        let file_names = Rc::new(RefCell::new(vec![]));

        Self {
            file_blobs,
            file_names,
            element,
        }
    }

    pub fn upload(&self, mut callback: impl FnMut(Vec<Blob>, Vec<String>) + 'static) {
        let element = self.element.clone();
        let blobs = self.file_blobs.clone();
        let names = self.file_names.clone();

        wasm_bindgen_futures::spawn_local(async move {
            let element = element.to_owned();
            let blobs_ref = blobs.to_owned();
            let names_ref = names.to_owned();

            let cb = Closure::wrap({
                let element = element.to_owned();

                Box::new(move |_: web_sys::Event| {
                    let element = element.clone();

                    let blobs_ref = blobs_ref.to_owned();
                    let names_ref = names_ref.to_owned();

                    let files = element.files().unwrap();
                    let len = files.length();
                    let mut names = vec![];
                    let mut blobs = vec![];

                    for i in 0..len {
                        let file = files.get(i).unwrap();
                        names.push(file.name());
                        blobs.push(file.into());
                    }

                    names_ref.replace(names);
                    blobs_ref.replace(blobs);

                    callback(blobs_ref.borrow().clone(), names_ref.borrow().clone());
                }) as Box<dyn FnMut(_)>
            });
            element
                .add_event_listener_with_callback("input", cb.as_ref().unchecked_ref())
                .unwrap();
            cb.forget();

            element.click();
        });
    }
}

impl Drop for FileUploader {
    fn drop(&mut self) {
        gloo::utils::document()
            .body()
            .unwrap()
            .remove_child(&self.element)
            .unwrap();
    }
}
