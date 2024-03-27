use stylist::yew::styled_component;
use wasm_bindgen::{closure::Closure, JsCast as _};
use yew::prelude::*;

use crate::{
    components::icons,
    functions::models::media::insert,
    utils::{global_state::GlobalStateContext, FileUploader},
};

#[styled_component]
pub fn Portal() -> HtmlResult {
    let global_state = use_context::<GlobalStateContext>().expect("Global state not found");

    let t = global_state.language.to_config().unwrap();

    let uploader = use_state(|| None);
    let file_blobs = use_state(Vec::new);
    let file_names: UseStateHandle<Vec<String>> = use_state(Vec::new);

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
                                    file_blobs.iter().enumerate().zip(file_names.iter())
                                        .map(|((index, blob), name)| {
                                            let src = web_sys::Url::create_object_url_with_blob(blob).unwrap();

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

                                                    <div
                                                        class={css!("
                                                            opacity: var(--show);
                                                            z-index: 1;
                                                        ")}
                                                    >
                                                        <icons::Delete />
                                                    </div>
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

                                        display: flex;
                                        align-items: center;
                                        justify-content: center;

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

                                            if let Some(uploader) = uploader.as_ref() {
                                                uploader.upload(move |blobs, names| {
                                                    file_blobs.set((*file_blobs).clone().into_iter().chain(blobs).collect());
                                                    file_names.set((*file_names).clone().into_iter().chain(names).collect());
                                                });
                                            }
                                        })
                                    }
                                >
                                    <icons::Upload color={"var(--color-dark-most)"} />
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
                                    let file_blobs = file_blobs.clone();

                                    Callback::from(move |_| {
                                        let file_blobs = file_blobs.to_owned();
                                        wasm_bindgen_futures::spawn_local(async move {
                                            for blob in (*file_blobs).clone().iter() {
                                                let reader = web_sys::FileReader::new().unwrap();
                                                let cb = Closure::wrap({
                                                    let reader = reader.to_owned();

                                                    Box::new(move |_: web_sys::Event| {
                                                        let reader = reader.to_owned();

                                                        wasm_bindgen_futures::spawn_local(async move {
                                                            let data = reader.result().unwrap();
                                                            let data = js_sys::Uint8Array::new(&data).to_vec();
                                                            log::warn!("{:?}", data.len());

                                                            match insert(data).await {
                                                                Ok(info) => {
                                                                    gloo::dialogs::alert(&format!("Success: {:?}", info));
                                                                }
                                                                Err(err) => {
                                                                    gloo::dialogs::alert(&format!("Fail: {:?}", err));
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
                            <icons::Upload size={48} color={"var(--icon-color-outside)"} />
                            { t.portal.upload }
                        </button>
                    }
                }
            }
        </div>
    })
}
