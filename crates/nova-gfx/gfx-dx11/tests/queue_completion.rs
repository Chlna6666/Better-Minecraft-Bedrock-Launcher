#![cfg(target_os = "windows")]

use gfx_core::{
    CommandDevice, CommandEncoderDescriptor, DeviceDescriptor, SubmissionDevice, SubmissionStatus,
};
use gfx_dx11::Dx11Device;

#[test]
#[ignore = "requires a Windows D3D11 hardware adapter"]
fn queue_idle_waits_for_prior_submission() {
    let mut device = Dx11Device::new(&DeviceDescriptor::default()).expect("hardware DX11 device");
    let encoder = device
        .create_command_encoder(&CommandEncoderDescriptor { label: None })
        .expect("encoder");
    let submission = device.submit_deferred(encoder).expect("submission");
    device.wait_queue_idle().expect("GPU queue idle");
    assert!(matches!(
        device.poll_submission(submission),
        Ok(SubmissionStatus::Complete)
    ));
    device
        .destroy_command_encoder(encoder)
        .expect("destroy encoder");
}
