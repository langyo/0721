use stylist::yew::styled_component;
use yew::prelude::*;

use crate::utils::FileUploader;
use _database::types::language_config::load_config;

#[styled_component]
pub fn Portal() -> HtmlResult {
    let t = load_config().unwrap();

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
                                    file_names.iter().zip(file_blobs.iter())
                                        .map(|(name, blob)| {
                                            let src = web_sys::Url::create_object_url_with_blob(&blob).unwrap();

                                            html! {
                                                <img
                                                    class={css!("
                                                        width: 128px;
                                                        height: 128px;
                                                        margin: 8px;

                                                        border-radius: 4px;
                                                        box-shadow: var(--shadow-half);
                                                        object-fit: cover;
                                                        user-select: none;
                                                    ")}
                                                    alt={name.clone()}
                                                    src={src}
                                                />
                                            }
                                        })
                                        .collect::<Html>()
                                }
                            </div>

                            <button
                                class={css!("
                                    width: 80%;
                                    max-width: 256px;
                                    height: 48px;

                                    font-size: 16px;
                                    font-weight: bolder;
                                ")}
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
