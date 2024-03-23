use stylist::css;
use yew::prelude::*;

use crate::functions::models::media::list;
use _database::types::config::{load_config, Config};

#[function_component]
pub fn Images() -> HtmlResult {
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

    let is_downloading = use_state(|| true);
    let image_list = use_state(|| Vec::new());
    use_effect_with((), {
        let is_downloading = is_downloading.clone();
        let image_list = image_list.clone();
        |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let image_list = image_list.clone();
                let ret = list(Some(0), Some(30)).await.unwrap();
                image_list.set(ret);

                is_downloading.set(false);
            });
        }
    });

    Ok(html! {
        <>
            {
                if *is_downloading {
                    html! {
                        <div class={css!("
                            position: fixed;
                            width: 100vw;
                            height: 100vh;
                            left: 0;
                            top: 0;

                            display: flex;
                            flex-direction: column;
                            align-items: center;
                            justify-content: center;
                        ")}>
                            <div class={css!("
                                @keyframes rotate {
                                    from {
                                        transform: rotate(0deg);
                                    }
                                    to {
                                        transform: rotate(360deg);
                                    }
                                }

                                width: 64px;
                                height: 64px;

                                border-radius: 50%;
                                border: 2px solid transparent;
                                border-top-color: var(--color-light-most);
                                border-bottom-color: var(--color-light-most);

                                animation: rotate 1s linear infinite;
                            ")} />
                        </div>
                    }
                } else {
                    html! {
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
                                image_list.iter().map(|item| html! {
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
                                            Callback::from(move |_| {
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
                                            src={format!("{}/{}?thumbnail=true", media_entry_path, item.hash)}
                                        />
                                    </div>
                                }).collect::<Html>()
                            }
                        </div>
                    }
                }
            }
        </>
    })
}
