use stylist::{css, yew::styled_component};
use yew::prelude::*;

use super::Props;

#[styled_component]
pub fn CircularProgress(props: &Props) -> Html {
    html! {
        <div
            class={css!("
                @keyframes rotate {
                    from {
                        transform: rotate(0deg);
                    }
                    to {
                        transform: rotate(360deg);
                    }
                }

                width: var(--size);
                height: var(--size);

                border-radius: 50%;
                border: 2px solid transparent;
                border-top-color: var(--icon-color);
                border-bottom-color: var(--icon-color);

                animation: rotate 1s linear infinite;
            ")}
            style={format!("
                --size: {}px;
                --icon-color: {};
            ", props.size, props.color)}
        />
    }
}
