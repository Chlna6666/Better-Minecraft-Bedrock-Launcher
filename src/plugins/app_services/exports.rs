//! Native dispatch generated from the same service catalogue as the Rust SDK.

use bmcbl_plugin_runtime::PluginError;

use super::{ServiceResult, encode};

macro_rules! evaluate {
    (direct($value:expr)) => {
        $value
    };
    (fallible($value:expr)) => {
        $value.map_err(|error| PluginError::invalid_input(error.to_string()))?
    };
}

macro_rules! host {
    ($($module:ident { $($(#[$doc:meta])* fn $method:ident(
        $($arg:ident : $ty:ty),*
    ) -> $result:ty = $service:literal => $kind:ident($target:expr);)*
    })*) => {
        pub(super) const fn names() -> &'static [&'static str] {
            &[$($($service,)*)*]
        }

        pub(super) async fn invoke(name: &str, bytes: &[u8]) -> ServiceResult {
            match name {
                $($($service => {
                    let (($($arg,)*), remaining): (($($ty,)*), _) = postcard::take_from_bytes(bytes)
                        .map_err(|error| PluginError::invalid_input(error.to_string()))?;
                    if !remaining.is_empty() {
                        return Err(PluginError::invalid_input("unexpected service arguments"));
                    }
                    let value: $result = evaluate!($kind(($target)($($arg),*)));
                    encode(&value)
                },)*)*
                _ => Err(PluginError::not_found("unknown public BMCBL service")),
            }
        }
    };
}
bmcbl_plugin_api::bmcbl_app_services!(host);


#[cfg(test)]
mod tests {
    use super::invoke;

    #[tokio::test]
    async fn invoke_linked_libraries() {
        let payload = b"plugin tile".as_slice();
        let input = postcard::to_allocvec(&(payload,)).unwrap();
        let output = invoke("bedrock-map-render.cache.tile-payload-fingerprint", &input)
            .await
            .unwrap();
        assert_eq!(
            postcard::from_bytes::<u128>(&output).unwrap(),
            bedrock_map_render::cache::tile_payload_fingerprint(payload)
        );

        let position = (-2_i32, 7_i32);
        let input = postcard::to_allocvec(&position).unwrap();
        let output = invoke("bedrock-world.query.is-slime-chunk", &input)
            .await
            .unwrap();
        assert_eq!(
            postcard::from_bytes::<bool>(&output).unwrap(),
            bedrock_world::query::is_bedrock_slime_chunk(position.0, position.1)
        );
    }
}
