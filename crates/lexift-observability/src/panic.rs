pub(crate) fn install() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        eprintln!("Lexift encountered an unrecoverable error: {info}");
        default_hook(info);
    }));
}
