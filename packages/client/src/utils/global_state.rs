use yew::prelude::*;

use _database::types::{i18n::Language, response::UserInfo};

#[derive(Debug, PartialEq, Clone)]
pub struct GlobalState {
    pub token: Option<UserInfo>,
    pub language: Language,
}

#[derive(Debug, PartialEq, Clone)]
pub enum GlobalStateAction {
    SetToken(Option<UserInfo>),
}

impl Reducible for GlobalState {
    type Action = GlobalStateAction;

    fn reduce(self: std::rc::Rc<Self>, action: Self::Action) -> std::rc::Rc<Self> {
        GlobalState {
            token: match action {
                GlobalStateAction::SetToken(token) => token,
            },
            language: self.language,
        }
        .into()
    }
}

pub type GlobalStateContext = UseReducerHandle<GlobalState>;

#[derive(Properties, Debug, PartialEq)]
pub struct GlobalStateProviderProps {
    pub language: Language,

    #[prop_or_default]
    pub children: Html,
}

#[function_component]
pub fn GlobalStateProvider(props: &GlobalStateProviderProps) -> Html {
    let state = use_reducer(|| GlobalState {
        token: None,
        language: props.language,
    });

    html! {
        <ContextProvider<GlobalStateContext> context={state}>
            {props.children.clone()}
        </ContextProvider<GlobalStateContext>>
    }
}
