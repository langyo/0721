use stylist::{css, yew::styled_component};
use yew::prelude::*;

use super::icons;

#[derive(Properties, Debug, PartialEq)]
pub struct Props {
    #[prop_or(false)]
    pub is_loading: bool,
}

#[styled_component]
pub fn GlobalSkeleton(props: &Props) -> Html {
    html! {
        <div class={classes!(css!("
            position: fixed;
            width: 100vw;
            height: 100vh;
            left: 0;
            top: 0;

            display: flex;
            flex-direction: column;
            align-items: center;
            justify-content: center;

            background: var(--color-dark-most);
            z-index: 1000;
            pointer-events: none;
            transition: all 0.3s;
        "), {
            if props.is_loading {
                css!("
                    opacity: 1;
                ")
            } else {
                css!("
                    opacity: 0;
                ")
            }
        })}>
            <icons::CircularProgress />
        </div>
    }
}
