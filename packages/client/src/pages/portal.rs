use stylist::yew::styled_component;
use wasm_bindgen::{closure::Closure, JsCast as _};
use yew::prelude::*;

use crate::{
    functions::{api::auth::refresh, models::media::insert},
    utils::FileUploader,
};
use _database::types::{language_config::load_config, response::AuthInfo};

#[styled_component]
pub fn Portal() -> HtmlResult {
    let t = load_config().unwrap();

    let auth = use_state(|| AuthInfo::None);

    use_effect_with((), {
        let auth = auth.clone();
        move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match refresh().await {
                    Ok(info) => {
                        auth.set(AuthInfo::User(info));
                    }
                    Err(_) => {
                        auth.set(AuthInfo::None);
                    }
                }
            });
            || {}
        }
    });

    let uploader = use_state(|| None);
    let file_blobs = use_state(|| vec![]);
    let file_names: UseStateHandle<Vec<String>> = use_state(|| vec![]);

    use_effect_with((), {
        let uploader = uploader.clone();

        move |_| {
            let uploader = uploader.to_owned();
            uploader.set(Some(FileUploader::new("image/*")));
        }
    });

    Ok(html! {
        <>
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
                                    file_blobs.iter().enumerate().zip(file_names.iter())
                                        .map(|((index, blob), name)| {
                                            let src = web_sys::Url::create_object_url_with_blob(&blob).unwrap();

                                            html! {
                                                <div
                                                    class={css!("
                                                        position: relative;
                                                        width: 128px;
                                                        height: 128px;
                                                        margin: 8px;

                                                        border-radius: 4px;
                                                        box-shadow: var(--shadow-half);

                                                        user-select: none;
                                                        cursor: pointer;

                                                        --image-filter: none;
                                                        --show: 0;

                                                        &:hover {
                                                            --image-filter: brightness(0.5) blur(4px);
                                                            --show: 1;
                                                        }
                                                    ")}
                                                    onclick={
                                                        let file_blobs = file_blobs.clone();
                                                        let file_names = file_names.clone();
                                                        Callback::from(move |_| {
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

                                                    <div class={css!("
                                                        position: absolute;
                                                        top: 50%;
                                                        left: 50%;
                                                        width: 24px;
                                                        height: 4px;

                                                        background: var(--color-light);
                                                        transform: translate(-50%, -50%) rotate(45deg);
                                                        opacity: var(--show, 0);
                                                    ")} />
                                                    <div class={css!("
                                                        position: absolute;
                                                        top: 50%;
                                                        left: 50%;
                                                        width: 24px;
                                                        height: 4px;

                                                        background: var(--color-light);
                                                        transform: translate(-50%, -50%) rotate(-45deg);
                                                        opacity: var(--show, 0);
                                                    ")} />
                                                </div>
                                            }
                                        })
                                        .collect::<Html>()
                                }

                                <button
                                    class={css!("
                                        position: relative;
                                        width: 48px;
                                        height: 48px;
                                        margin: 8px;
                                        margin-left: calc(8px + (128px - 48px) / 2);

                                        border: none;
                                        border-radius: 4px;
                                        box-shadow: var(--shadow-half);
                                        background: var(--color-light-most);

                                        cursor: pointer;

                                        &:hover {
                                            background: var(--color-light);
                                        }
                                    ")}
                                    onclick={
                                        let uploader = uploader.clone();
                                        let file_blobs = file_blobs.clone();
                                        let file_names = file_names.clone();

                                        Callback::from(move |_| {
                                            let uploader = uploader.to_owned();
                                            let file_blobs = file_blobs.to_owned();
                                            let file_names = file_names.to_owned();

                                            uploader.as_ref().map(|uploader| {
                                                uploader.upload(move |blobs, names| {
                                                    file_blobs.set((*file_blobs).clone().into_iter().chain(blobs).collect());
                                                    file_names.set((*file_names).clone().into_iter().chain(names).collect());
                                                });
                                            });
                                        })
                                    }
                                >
                                    <div class={css!("
                                        position: absolute;
                                        top: 50%;
                                        left: 50%;
                                        width: 24px;
                                        height: 4px;
                                        background: var(--color-dark);
                                        transform: translate(-50%, -50%);
                                    ")} />
                                    <div class={css!("
                                        position: absolute;
                                        top: 50%;
                                        left: 50%;
                                        width: 4px;
                                        height: 24px;
                                        background: var(--color-dark);
                                        transform: translate(-50%, -50%);
                                    ")} />
                                </button>
                            </div>

                            <button
                                class={css!("
                                    width: 80%;
                                    max-width: 256px;
                                    height: 48px;

                                    font-size: 16px;
                                    font-weight: bolder;
                                ")}
                                onclick={
                                    let token = auth.clone();
                                    let file_blobs = file_blobs.clone();

                                    Callback::from(move |_| {
                                        let token = match (*token).clone() {
                                            AuthInfo::User(token) => token,
                                            AuthInfo::None => {
                                                gloo::dialogs::alert("请先登录");
                                                return;
                                            }
                                        };

                                        let file_blobs = file_blobs.to_owned();
                                        wasm_bindgen_futures::spawn_local(async move {
                                            let token = token.to_owned();
                                            for blob in (*file_blobs).clone().iter() {
                                                let token = token.to_owned();
                                                let reader = web_sys::FileReader::new().unwrap();
                                                let cb = Closure::wrap({
                                                    let token = token.to_owned();
                                                    let reader = reader.to_owned();

                                                    Box::new(move |_: web_sys::Event| {
                                                        let token = token.to_owned();
                                                        let reader = reader.to_owned();

                                                        wasm_bindgen_futures::spawn_local(async move {
                                                            let token = token.to_owned();
                                                            let data = reader.result().unwrap();
                                                            let data = js_sys::Uint8Array::new(&data).to_vec();
                                                            log::warn!("{:?}", data.len());

                                                            match insert(token, data).await {
                                                                Ok(info) => {
                                                                    gloo::dialogs::alert(&format!("上传成功：{:?}", info));
                                                                }
                                                                Err(err) => {
                                                                    gloo::dialogs::alert(&format!("上传失败：{:?}", err));
                                                                }
                                                            }
                                                        });
                                                    }) as Box<dyn FnMut(_)>
                                                });

                                                reader
                                                    .add_event_listener_with_callback(
                                                        "loadend",
                                                        &cb.as_ref().unchecked_ref(),
                                                    )
                                                    .unwrap();
                                                cb.forget();
                                                reader.read_as_array_buffer(&blob).unwrap();
                                            }
                                        });
                                    })
                                }
                            >
                                { t.portal.upload }
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

                                border: 4px dashed var(--color-dark-half);
                                border-radius: 4px;
                                box-shadow: var(--shadow-half);

                                background: var(--color-light-half);
                                color: var(--color-dark-most);
                                font-size: 24px;
                                font-weight: bolder;
                            ")}
                            onclick={
                                let uploader = uploader.clone();
                                let file_blobs = file_blobs.clone();
                                let file_names = file_names.clone();

                                Callback::from(move |_| {
                                    let uploader = uploader.to_owned();
                                    let file_blobs = file_blobs.to_owned();
                                    let file_names = file_names.to_owned();

                                    uploader.as_ref().map(|uploader| {
                                        uploader.upload(move |blobs, names| {
                                            file_blobs.set(blobs);
                                            file_names.set(names);
                                        });
                                    });
                                })
                            }
                        >
                            { t.portal.upload }
                        </button>
                    }
                }
            }
        </>
    })
}
