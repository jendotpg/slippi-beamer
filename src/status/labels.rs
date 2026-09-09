#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorLabel {
    NoId,
    NoSdCard,
    SdUnreadable,
    WrongFormat,
    NoConfig,
    BadConfig,
    NoUsb,
    NoWifi,
    NoHttp,
    NoMdns,
    OutOfMemory,
    Crashed,
}

impl ErrorLabel {
    pub fn is_storage_fault(self) -> bool {
        matches!(
            self,
            ErrorLabel::NoSdCard | ErrorLabel::SdUnreadable | ErrorLabel::NoUsb
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ErrorLabel::NoId => "NO ID",
            ErrorLabel::NoSdCard => "NO SD CARD",
            ErrorLabel::SdUnreadable => "SD UNREADABLE",
            ErrorLabel::WrongFormat => "WRONG FORMAT",
            ErrorLabel::NoConfig => "NO CONFIG",
            ErrorLabel::BadConfig => "BAD CONFIG",
            ErrorLabel::NoUsb => "NO USB",
            ErrorLabel::NoWifi => "NO WIFI",
            ErrorLabel::NoHttp => "NO HTTP",
            ErrorLabel::NoMdns => "NO MDNS",
            ErrorLabel::OutOfMemory => "OUT OF MEMORY",
            ErrorLabel::Crashed => "CRASHED",
        }
    }
}

impl core::fmt::Display for ErrorLabel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningLabel {
    DriveFailing,
    DriveFull,
    NoHost,
    SlpMisformat,
    DriveFilling,
    WeakLink,
    WifiNotAssociated,
    WifiNoDHCPLease,
    LowMemory,
}

pub const WARNINGS: [WarningLabel; 9] = [
    WarningLabel::DriveFailing,
    WarningLabel::LowMemory,
    WarningLabel::DriveFull,
    WarningLabel::NoHost,
    WarningLabel::WifiNotAssociated,
    WarningLabel::WifiNoDHCPLease,
    WarningLabel::WeakLink,
    WarningLabel::SlpMisformat,
    WarningLabel::DriveFilling,
];

impl WarningLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            WarningLabel::DriveFailing => "DRIVE FAILING",
            WarningLabel::DriveFull => "DRIVE FULL",
            WarningLabel::NoHost => "NO WII",
            WarningLabel::SlpMisformat => "SLP MISFORMAT",
            WarningLabel::DriveFilling => "DRIVE FILLING",
            WarningLabel::WeakLink => "WEAK LINK",
            WarningLabel::WifiNotAssociated => "WIFI ISSUE",
            WarningLabel::WifiNoDHCPLease => "WIFI TOO FULL",
            WarningLabel::LowMemory => "LOW MEMORY",
        }
    }

    pub fn reason(self) -> &'static str {
        match self {
            WarningLabel::DriveFailing => "cannot read the card",
            WarningLabel::DriveFull => "new replays are not served",
            WarningLabel::NoHost => "not plugged into wii or pc",
            WarningLabel::SlpMisformat => "a replay is formatted incorrectly",
            WarningLabel::DriveFilling => "delete replays from the card soon",
            WarningLabel::WeakLink => "the wifi connection is poor",
            WarningLabel::WifiNotAssociated => "cannot reach the wifi network",
            WarningLabel::WifiNoDHCPLease => "the network gave out no address",
            WarningLabel::LowMemory => "not enough heap left to serve safely",
        }
    }

    pub(crate) fn bit(self) -> u32 {
        1 << (self as u32)
    }
}

impl core::fmt::Display for WarningLabel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}
