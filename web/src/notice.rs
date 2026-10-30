//! One-line alerts and status notes, composed from the library's `Item`.

use dioxus::prelude::*;

use crate::components::item::{Item, ItemContent, ItemTitle, ItemVariant};

/// A problem the owner must see, announced to assistive technology as an alert.
#[component]
pub fn Alert(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        Item { variant: ItemVariant::Outline, role: "alert", attributes,
            ItemContent { ItemTitle { {children} } }
        }
    }
}

/// A neutral state of the page, announced politely as a status.
#[component]
pub fn Status(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        Item { variant: ItemVariant::Muted, role: "status", attributes,
            ItemContent { ItemTitle { {children} } }
        }
    }
}
