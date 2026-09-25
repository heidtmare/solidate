//! `sqlx` mappings for enums stored as `text`.

use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::postgres::{PgArgumentBuffer, PgHasArrayType, PgTypeInfo, PgValueRef};
use sqlx::{Decode, Encode, Postgres, Type};

use crate::auth::{Role, Scope};
use crate::sync::Variant;

macro_rules! text_type {
    ($($ty:ty),*) => {$(
        impl Type<Postgres> for $ty {
            fn type_info() -> PgTypeInfo {
                <String as Type<Postgres>>::type_info()
            }
            fn compatible(ty: &PgTypeInfo) -> bool {
                <String as Type<Postgres>>::compatible(ty)
            }
        }

        impl PgHasArrayType for $ty {
            fn array_type_info() -> PgTypeInfo {
                <String as PgHasArrayType>::array_type_info()
            }
        }

        impl Encode<'_, Postgres> for $ty {
            fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
                <&str as Encode<Postgres>>::encode(self.as_str(), buf)
            }
        }

        impl<'r> Decode<'r, Postgres> for $ty {
            fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
                Ok(<&str as Decode<Postgres>>::decode(value)?.parse()?)
            }
        }
    )*};
}

text_type!(Variant, Role, Scope);
