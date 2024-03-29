use std::{cell::RefCell, rc::Rc};

use stylist::yew::styled_component;
use wasm_bindgen::{closure::Closure, JsCast as _};
use yew::prelude::*;

use crate::{
    components::icons,
    functions::models::media::insert,
    utils::{copy_to_clipboard, global_state::GlobalStateContext, FileUploader},
};
use _database::types::config::{load_config, Config};

#[derive(Debug, Clone, PartialEq)]
pub enum UploadStatus {
    Ready,
    Uploading,
    Success(String),
    Fail,
}

#[styled_component]
pub fn Portal() -> HtmlResult {
    let global_config = use_prepared_state!((), async move |_| -> Option<Config> {
        if let Ok(ret) = load_config() {
            return Some(ret);
        }
        None
    })?
    .unwrap();
    let media_entry_path = (*global_config)
        .clone()
        .map(|config| config.router.media_entry_path)
        .unwrap_or("/media".to_string());

    let global_state = use_context::<GlobalStateContext>().expect("Global state not found");

    let t = global_state.language.to_config().unwrap();

    let uploader = use_state(|| None);
    let file_blobs = use_state(Vec::new);
    let file_names: UseStateHandle<Vec<String>> = use_state(Vec::new);
    let upload_status: UseStateHandle<Vec<UploadStatus>> = use_state(Vec::new);

    use_effect_with((), {
        let uploader = uploader.clone();

        move |_| {
            let uploader = uploader.to_owned();
            uploader.set(Some(FileUploader::new("image/*")));
        }
    });

    Ok(html! {
        <div class={css!("
            position: relative;
            width: 100%;
            margin-top: 96px;
            margin-bottom: 64px;
            padding: 64px;

            display: flex;
            flex-direction: column;
            align-items: center;
            justify-content: center;
        ")}>
            <div class={css!("
                position: fixed;
                width: 100vw;
                height: 100vh;
                left: 0;
                top: 0;

                background: url('/bg.webp') center / cover no-repeat;
                filter: blur(4px);
                z-index: -1;
                pointer-events: none;
            ")} />

            {
                if !file_names.is_empty() {
                    html! {
                        <>
                            <div class={css!("
                                width: 80%;
                                margin: 32px;
                                padding: 32px;

                                background: var(--color-light-half);
                                border-radius: 4px;
                                color: var(--color-dark-most);
                                font-size: 16px;
                                font-weight: bolder;

                                display: flex;
                                align-items: center;
                                justify-content: center;
                            ")}>
                                {
                                    file_blobs.iter().take(6).enumerate().zip(file_names.iter()).zip(upload_status.iter())
                                        .map(|(((index, blob), name), status)| {
                                            let src = web_sys::Url::create_object_url_with_blob(blob).unwrap();
                                            let status = status.clone();

                                            html! {
                                                <div
                                                    class={css!("
                                                        position: relative;
                                                        width: 128px;
                                                        height: 128px;
                                                        margin: 8px;

                                                        border-radius: 4px;
                                                        box-shadow: var(--shadow-half);

                                                        display: flex;
                                                        align-items: center;
                                                        justify-content: center;

                                                        user-select: none;
                                                        cursor: pointer;

                                                        --image-filter: none;
                                                        --show: 0;
                                                        --not-show: 1;

                                                        &:hover {
                                                            --image-filter: brightness(0.5) blur(4px);
                                                            --show: 1;
                                                            --not-show: 0;
                                                        }
                                                    ")}
                                                    onclick={
                                                        let file_blobs = file_blobs.clone();
                                                        let file_names = file_names.clone();
                                                        let status = status.clone();
                                                        let media_entry_path = media_entry_path.clone();

                                                        Callback::from(move |_| {
                                                            match status.to_owned() {
                                                                UploadStatus::Success(hash) => {
                                                                    let origin = gloo::utils::document().location().unwrap().origin().unwrap();
                                                                    let media_entry_path = media_entry_path.clone();
                                                                    copy_to_clipboard(format!("{}{}/{}", origin, media_entry_path, hash));

                                                                    gloo::dialogs::alert(&format!("Copied: {}", hash));
                                                                },
                                                                UploadStatus::Uploading => {},
                                                                _ => {
                                                                    file_blobs.set(
                                                                        file_blobs.iter().enumerate().filter_map(
                                                                            |(i, b)| if i == index { None } else { Some(b.clone()) }
                                                                        ).collect()
                                                                    );
                                                                    file_names.set(
                                                                        file_names.iter().enumerate().filter_map(
                                                                            |(i, n)| if i == index { None } else { Some(n.clone()) }
                                                                        ).collect()
                                                                    );
                                                                },
                                                            }
                                                        })
                                                    }
                                                >
                                                    <img
                                                        class={css!("
                                                            position: absolute;
                                                            top: 0;
                                                            left: 0;
                                                            width: 100%;
                                                            height: 100%;

                                                            object-fit: cover;
                                                            border-radius: 4px;
                                                            filter: var(--image-filter);
                                                        ")}
                                                        alt={name.clone()}
                                                        src={src}
                                                    />

                                                    <div
                                                        class={css!("
                                                            opacity: var(--show);
                                                            z-index: 1;
                                                        ")}
                                                    >
                                                        {
                                                            match status {
                                                                UploadStatus::Success(_) => html! {
                                                                    <icons::Copy />
                                                                },
                                                                UploadStatus::Uploading => html! {},
                                                                _ => html! {
                                                                    <icons::Delete />
                                                                },
                                                            }
                                                        }
                                                    </div>

                                                    <div
                                                        class={classes!(css!("
                                                            position: absolute;
                                                            width: 100%;
                                                            height: 100%;
                                                            top: 0;
                                                            left: 0;

                                                            background: var(--color-dark-most);
                                                            border-radius: 4px;

                                                            display: flex;
                                                            align-items: center;
                                                            justify-content: center;

                                                            z-index: 1;
                                                            pointer-events: none;
                                                        "), {
                                                            match status {
                                                                UploadStatus::Ready => css!("
                                                                    opacity: 0;
                                                                "),
                                                                _ => css!("
                                                                    opacity: var(--not-show);
                                                                "),
                                                            }
                                                        })}
                                                    >
                                                        {
                                                            match status {
                                                                UploadStatus::Success(_info) => html! {
                                                                    <icons::Check color={"var(--color-light-most)"} />
                                                                },
                                                                UploadStatus::Uploading => html! {
                                                                    <icons::CircularProgress color={"var(--color-light-most)"} />
                                                                },
                                                                UploadStatus::Fail => html! {
                                                                    <icons::Close color={"var(--color-light-most)"} />
                                                                },
                                                                _ => html! {},
                                                            }
                                                        }
                                                    </div>
                                                </div>
                                            }
                                        })
                                        .collect::<Html>()
                                }

                                <button
                                    class={classes!(css!("
                                        position: relative;
                                        margin: 8px;

                                        border: none;
                                        border-radius: 4px;
                                        box-shadow: var(--shadow-half);
                                        background: var(--color-light-most);

                                        display: flex;
                                        align-items: center;
                                        justify-content: center;

                                        font-size: 24px;
                                        font-weight: bolder;
                                        color: var(--color-dark-most);

                                        user-select: none;
                                        cursor: pointer;

                                        &:hover {
                                            background: var(--color-light);
                                            color: var(--color-dark);
                                        }
                                    "), {
                                        if file_blobs.len() > 6 {
                                            css!("
                                                width: 96px;
                                                height: 96px;
                                                margin-left: calc(8px + (128px - 96px) / 2);
                                            ")
                                        } else {
                                            css!("
                                                width: 64px;
                                                height: 64px;
                                                margin-left: calc(8px + (128px - 64px) / 2);
                                            ")
                                        }
                                    }, {
                                        if upload_status.iter().all(|status|
                                            match status {
                                                UploadStatus::Uploading => false,
                                                _ => true
                                            }
                                        ) {
                                            css!("")
                                        } else {
                                            css!("
                                                background: var(--color-dark-less);
                                                pointer-events: none;
                                            ")
                                        }
                                    })}
                                    onclick={
                                        let uploader = uploader.clone();
                                        let file_blobs = file_blobs.clone();
                                        let file_names = file_names.clone();

                                        Callback::from(move |_| {
                                            let uploader = uploader.to_owned();
                                            let file_blobs = file_blobs.to_owned();
                                            let file_names = file_names.to_owned();

                                            if let Some(uploader) = uploader.as_ref() {
                                                uploader.upload(move |blobs, names| {
                                                    file_blobs.set(blobs);
                                                    file_names.set(names);
                                                });
                                            }
                                        })
                                    }
                                >
                                    {
                                        if file_blobs.len() > 6 {
                                            html! {
                                                {format!("+{}", file_blobs.len() - 6)}
                                            }
                                        } else {
                                            html! {
                                                <icons::Upload color={"var(--color-dark-most)"} />
                                            }
                                        }
                                    }
                                </button>
                            </div>

                            <button
                                class={classes!(css!("
                                    width: 80%;
                                    max-width: 256px;
                                    height: 48px;

                                    font-size: 16px;
                                    font-weight: bolder;
                                "), {
                                    if upload_status.iter().all(|status|
                                        match status {
                                            UploadStatus::Uploading => false,
                                            _ => true
                                        }
                                    ) {
                                        css!("")
                                    } else {
                                        css!("
                                            background: var(--color-dark-most);
                                            color: var(--color-light-most);
                                            pointer-events: none;
                                        ")
                                    }
                                })}
                                onclick={
                                    let file_blobs = file_blobs.clone();
                                    let upload_status = upload_status.clone();

                                    Callback::from(move |_| {
                                        let file_blobs = file_blobs.to_owned();
                                        let upload_status = upload_status.to_owned();

                                        let len = file_blobs.len();
                                        let init_status = (0..len).map(|_| UploadStatus::Uploading).collect::<Vec<_>>();
                                        upload_status.set(init_status.clone());

                                        wasm_bindgen_futures::spawn_local(async move {
                                            let status = Rc::new(RefCell::new(init_status));

                                            for (index, blob) in (*file_blobs).clone().iter().enumerate() {
                                                let upload_status = upload_status.clone();
                                                let reader = web_sys::FileReader::new().unwrap();
                                                let status = status.clone();

                                                let cb = Closure::wrap({
                                                    let reader = reader.to_owned();
                                                    let upload_status = upload_status.to_owned();
                                                    let status = status.to_owned();

                                                    Box::new(move |_: web_sys::Event| {
                                                        let reader = reader.to_owned();
                                                        let upload_status = upload_status.to_owned();
                                                        let status = status.to_owned();

                                                        wasm_bindgen_futures::spawn_local(async move {
                                                            let data = reader.result().unwrap();
                                                            let data = js_sys::Uint8Array::new(&data).to_vec();
                                                            log::warn!("{:?}", data.len());

                                                            match insert(data).await {
                                                                Ok(info) => {
                                                                    log::info!("{:?}", info);
                                                                    status.borrow_mut()[index] = UploadStatus::Success(info);
                                                                    upload_status.set((*status.borrow()).clone());
                                                                }
                                                                Err(err) => {
                                                                    log::error!("{:?}", err);
                                                                    status.borrow_mut()[index] = UploadStatus::Fail;
                                                                    upload_status.set((*status.borrow()).clone());
                                                                }
                                                            }
                                                        });
                                                    }) as Box<dyn FnMut(_)>
                                                });

                                                reader
                                                    .add_event_listener_with_callback(
                                                        "loadend",
                                                        cb.as_ref().unchecked_ref(),
                                                    )
                                                    .unwrap();
                                                cb.forget();
                                                reader.read_as_array_buffer(blob).unwrap();
                                            }
                                        });
                                    })
                                }
                            >
                                {
                                    if upload_status.iter().all(|status|
                                        match status {
                                            UploadStatus::Uploading => false,
                                            _ => true
                                        }
                                    ) {
                                        html! {
                                            {t.portal.upload}
                                        }
                                    } else {
                                        html! {
                                            <div class={css!("
                                                width: 100%;
                                                height: 100%;
                                                padding: 0 16px;

                                                display: flex;
                                                align-items: center;
                                                justify-content: space-between;
                                            ")}>
                                                <icons::CircularProgress size={12} color={"var(--color-light-most)"} />

                                                {format!(
                                                    "{} {} / {}",
                                                    t.portal.progress,
                                                    upload_status.iter().filter(|status|
                                                        match status {
                                                            UploadStatus::Uploading => false,
                                                            _ => true
                                                        }
                                                    ).count(),
                                                    upload_status.len()
                                                )}
                                            </div>
                                        }
                                    }
                                }
                            </button>
                        </>
                    }
                } else {
                    html! {
                        <button
                            class={css!("
                                width: 80%;
                                max-width: 256px;
                                height: 128px;
                                padding: 16px 0;

                                border: 4px dashed var(--color-dark-half);
                                border-radius: 4px;
                                box-shadow: var(--shadow-half);

                                background: var(--color-light-half);
                                color: var(--color-dark-most);
                                font-size: 24px;
                                font-weight: bolder;

                                display: flex;
                                flex-direction: column;
                                align-items: center;
                                justify-content: space-between;

                                --icon-color-outside: var(--color-dark-most);

                                &:hover {
                                    --icon-color-outside: var(--color-light-most);
                                }
                            ")}
                            onclick={
                                let uploader = uploader.clone();
                                let file_blobs = file_blobs.clone();
                                let file_names = file_names.clone();
                                let upload_status = upload_status.clone();

                                Callback::from(move |_| {
                                    let uploader = uploader.to_owned();
                                    let file_blobs = file_blobs.to_owned();
                                    let file_names = file_names.to_owned();
                                    let upload_status = upload_status.to_owned();

                                    if let Some(uploader) = uploader.as_ref() {
                                        uploader.upload(move |blobs, names| {
                                            let len = blobs.len();
                                            file_blobs.set(blobs);
                                            file_names.set(names);
                                            upload_status.set((0..len).map(|_| UploadStatus::Ready).collect());
                                        });
                                    }
                                })
                            }
                        >
                            <icons::Upload size={48} color={"var(--icon-color-outside)"} />
                            { t.portal.upload }
                        </button>
                    }
                }
            }
        </div>
    })
}
