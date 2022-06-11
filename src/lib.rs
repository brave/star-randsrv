//! Foreign-function interface to the ppoprf randomness implementation
//!
//! This implements a C api so services can embed support.
//!

use ppoprf::ppoprf;

/// Opaque struct acts as a handle to the server implementation.
pub struct RandomnessServer {
    inner: ppoprf::Server,
}

/// Construct a new server instance and return an opaque handle to it.
///
/// # Safety
///
/// The returned pointer is allocated by Rust and must be freed by
/// the same allocator. The handle should be passed to the correspoding
/// randomness_server_release() function to release the associated
/// memory.
///
/// If initialization of the service instance fails, a null pointer
/// is returned. The caller should check for this an handle the error
/// accordingly.
// FIXME: Pass a [u8] and length for the md initialization.
#[no_mangle]
pub extern "C" fn randomness_server_create() -> *mut RandomnessServer {
    let test_mds = vec![0u8];
    if let Ok(inner) = ppoprf::Server::new(test_mds) {
        let server = Box::new(RandomnessServer { inner });
        Box::into_raw(server)
    } else {
        // Server creation failed; return nullptr.
        std::ptr::null_mut()
    }
}

/// Release memory associated with a server instance.
///
/// Pass the handle returned by randomness_server_create() to this
/// function to release the associated resources.
///
/// # Safety
///
/// The `ptr` argument must point to a valid RandomnessServer instance
/// allocated by Rust.
#[no_mangle]
pub unsafe extern "C" fn randomness_server_release(ptr: *mut RandomnessServer) {
    assert!(!ptr.is_null());
    let server = Box::from_raw(ptr);
    drop(server);
}

/// Evaluate the PPOPRF for the given point.
///
/// # Safety
///
/// The `ptr` argument must point to a valid RandomnessServer state
/// struct, such as is returned by randomness_server_create().
///
/// The `input` and `output` arguments must point to accessible areas
/// of memory with the correct amount of space available.
#[no_mangle]
pub unsafe extern "C" fn randomness_server_eval(
    ptr: *const RandomnessServer,
    input: *const u8,
    md: u8,
    verifiable: bool,
    output: *mut u8,
) -> bool {
    // Verify arguments.
    assert!(!ptr.is_null());
    assert!(!input.is_null());
    assert!(!output.is_null());

    // Convert our *const argument to a &ppoprf::Server without taking ownership.
    let server = &(*ptr).inner;
    // Wrap the provided compressed Ristretto point in the expected type.
    // Unfortunately from_slice() copies the data here.
    let input = std::slice::from_raw_parts(input, ppoprf::COMPRESSED_POINT_LEN);
    if let Ok(point) = serde_json::from_slice(input) {
        // Evaluate the requested point.
        if let Ok(result) = server.eval(&point, md, verifiable) {
            // Copy the resulting point into the output buffer.
            std::ptr::copy_nonoverlapping(
                result.output.as_bytes().as_ptr(),
                output,
                ppoprf::COMPRESSED_POINT_LEN,
            );
            // success
            return true;
        }
    }
    // Earlier code failed.
    false
}

/// Puncture the given md value from the PPOPRF.
///
/// # Safety
///
/// The `ptr` argument must point to a valid RandomnessServer state
/// struct, such as is returned by randomness_server_create().
#[no_mangle]
pub unsafe extern "C" fn randomness_server_puncture(ptr: *mut RandomnessServer, md: u8) -> bool {
    // Convert our *const to a &ppoprf::Server without taking ownership.
    assert!(!ptr.is_null());
    let server = &mut (*ptr).inner;

    // Call correct function.
    server.puncture(md).is_ok()
}

/// # Safety
///
/// The `ptr` argument must point to a valid RandomnessServer state
/// struct, such as is returned by randomness_server_create().
#[no_mangle]
pub unsafe extern "C" fn randomness_server_get_public_key(ptr: *const RandomnessServer, output: *mut u8) -> usize {
    // Convert our *const to a &ppoprf::Server without taking ownership.
    assert!(!ptr.is_null());
    let server = &(*ptr).inner;

    if let Ok(data) = server.get_public_key().serialize_to_bincode() {
        std::ptr::copy_nonoverlapping(
            data.as_ptr(),
            output,
            data.len()
        );
        return data.len();
    }
    0
}

#[cfg(test)]
mod tests {
    //! Unit tests for the ppoprf foreign-function interface
    //!
    //! This tests the C-compatible api from Rust for convenience.
    //! Testing it from other langauges is also recommended!

    use crate::*;
    use curve25519_dalek::ristretto::CompressedRistretto;

    #[test]
    /// Verify creation/release of the opaque server handle.
    fn unused_instance() {
        let server = randomness_server_create();
        assert!(!server.is_null());
        unsafe {
            randomness_server_release(server);
        }
    }

    #[test]
    /// One evaluation call to the ppoprf.
    fn simple_eval() {
        let server = randomness_server_create();
        assert!(!server.is_null());

        // Evaluate a test point.
        let point = CompressedRistretto::default();
        let mut result = Vec::with_capacity(ppoprf::COMPRESSED_POINT_LEN);
        unsafe {
            randomness_server_eval(
                server,
                point.as_bytes().as_ptr(),
                0,
                false,
                result.as_mut_ptr(),
            );
            // FIXME: verify result!
            randomness_server_release(server);
        }
    }

    #[test]
    /// Verify serialization of internal types.
    fn serialization() {
        let point = CompressedRistretto::default();
        println!("{:?}", &point);

        // ppoprf::Evaluation doesn't implement Debug.
    }
}
