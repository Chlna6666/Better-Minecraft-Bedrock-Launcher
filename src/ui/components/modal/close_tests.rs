use super::*;

struct DialogState {
    open: bool,
}

impl Render for DialogState {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

struct DialogHost {
    state: Entity<DialogState>,
    dismiss: ModalDismissHandle,
    cleanup_ran: Rc<Cell<bool>>,
    closed_frames: Rc<Cell<usize>>,
}

impl Render for DialogHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div().size_full().child(self.state.clone());
        if self.state.read(cx).open {
            let state = self.state.clone();
            let cleanup = self.cleanup_ran.clone();
            let cleanup_before_dismiss = cleanup.clone();
            root = root.child(modal_layer_dismissible_with_handle_and_cleanup(
                self.dismiss.clone(),
                div().size(px(40.0)).bg(white()),
                black(),
                Rc::new(move |_| cleanup.set(true)),
                Rc::new(move |cx| {
                    assert!(cleanup_before_dismiss.get());
                    state.update(cx, |state, cx| {
                        state.open = false;
                        cx.notify();
                    });
                }),
            ));
        } else {
            self.closed_frames.set(self.closed_frames.get() + 1);
        }
        root
    }
}

#[gpui::test]
fn close_completion_redraws_host_without_another_input(cx: &mut TestAppContext) {
    let (host, cx) = cx.add_window_view(|_, cx| DialogHost {
        state: cx.new(|_| DialogState { open: true }),
        dismiss: ModalDismissHandle::new(),
        cleanup_ran: Rc::new(Cell::new(false)),
        closed_frames: Rc::new(Cell::new(0)),
    });

    host.update(cx, |host, cx| {
        let mut control = host.dismiss.control.get();
        control.closing_started_at = Some(Instant::now() - Duration::from_secs(1));
        control.closing_from = 1.0;
        host.dismiss.control.set(control);
        cx.notify();
    });
    cx.run_until_parked();

    host.read_with(cx, |host, cx| {
        assert!(host.cleanup_ran.get());
        assert!(!host.state.read(cx).open);
        assert!(
            host.closed_frames.get() > 0,
            "closing must redraw the host to remove the modal backdrop without user input"
        );
    });
}
