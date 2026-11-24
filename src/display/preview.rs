use super::*;

impl Display {
    pub(crate) fn for_mapping_capture(shot: Shot, maps: PathBuf) -> Self {
        let (input, _) = mpsc::unbounded_channel();
        let state = Shared {
            latest: Some(Frame {
                format: Some(Format {
                    width: shot.width,
                    height: shot.height,
                    ..Default::default()
                }),
                pixels: shot.pixels,
            }),
            ..Default::default()
        };
        let mut display = Self::new(Arc::new(Mutex::new(state)), input);
        display.mapping.load(maps);
        display
    }
}
