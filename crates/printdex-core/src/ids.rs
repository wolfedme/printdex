//! Typed entity ids. Every id is a UUID v7, so ids sort by creation time.

use std::fmt;

pub use uuid::Uuid;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, sqlx::Type)]
        #[sqlx(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// A new, time-ordered (v7) id.
            #[allow(clippy::new_without_default)]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0.hyphenated(), f)
            }
        }
    };
}

id_type!(
    /// A registered library folder.
    LibraryId
);
id_type!(
    /// A model: a folder of parts, or a single printable file.
    ModelId
);
id_type!(
    /// A printable file (STL, 3MF, OBJ) belonging to a model.
    PartId
);
id_type!(
    /// A non-printable file inside a model folder (image, readme, ...).
    AssetId
);
id_type!(
    /// A tag (flat, case-insensitive).
    TagId
);
id_type!(
    /// A hand-picked list of models.
    CollectionId
);
id_type!(
    /// One print attempt of a model.
    AttemptId
);
id_type!(
    /// A photo attached to a print attempt.
    PhotoId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_v7_and_sortable() {
        let a = ModelId::new();
        let b = ModelId::new();
        assert_eq!(a.0.get_version_num(), 7);
        assert!(a <= b);
    }
}
