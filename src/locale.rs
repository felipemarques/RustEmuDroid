#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, clap::ValueEnum)]
#[cfg_attr(feature = "ui", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "ui", serde(rename_all = "kebab-case"))]
pub(crate) enum Locale {
    #[default]
    PtBr,
    EnUs,
    EsEs,
}

impl Locale {
    #[cfg(feature = "ui")]
    pub(crate) const ALL: [Self; 3] = [Self::PtBr, Self::EnUs, Self::EsEs];

    #[cfg(feature = "ui")]
    pub(crate) const fn tag(self) -> &'static str {
        self.pick("pt-br", "en-us", "es-es")
    }

    #[cfg(feature = "ui")]
    pub(crate) const fn name(self) -> &'static str {
        self.pick("Português (Brasil)", "English (US)", "Español (España)")
    }

    pub(crate) const fn pick(
        self,
        pt_br: &'static str,
        en_us: &'static str,
        es_es: &'static str,
    ) -> &'static str {
        match self {
            Self::PtBr => pt_br,
            Self::EnUs => en_us,
            Self::EsEs => es_es,
        }
    }
}
