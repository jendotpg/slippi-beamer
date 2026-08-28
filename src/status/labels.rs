#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    NoId,
    NoSdCard,
    SdUnreadable,
    WrongFormat,
    NoConfig,
    BadConfig,
    NoUsb,
    NoWifi,
    WrongWifi,
    NoIp,
    NoHttp,
    NoMdns,
    Crashed,
}

impl Label {
    pub fn is_storage_fault(self) -> bool {
        matches!(self, Label::NoSdCard | Label::SdUnreadable | Label::NoUsb)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Label::NoId => "NO ID",
            Label::NoSdCard => "NO SD CARD",
            Label::SdUnreadable => "SD UNREADABLE",
            Label::WrongFormat => "WRONG FORMAT",
            Label::NoConfig => "NO CONFIG",
            Label::BadConfig => "BAD CONFIG",
            Label::NoUsb => "NO USB",
            Label::NoWifi => "NO WIFI",
            Label::WrongWifi => "WRONG WIFI",
            Label::NoIp => "NO IP",
            Label::NoHttp => "NO HTTP",
            Label::NoMdns => "NO MDNS",
            Label::Crashed => "CRASHED",
        }
    }
}

impl core::fmt::Display for Label {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}
