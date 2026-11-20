//! 執行期錯誤的錯誤碼。
//!
//! 規格（`fluxa/docs/Fluxa語言規格v0.1.md` §6.2）的錯誤碼以靜態檢查為主；模擬期抓到的錯誤
//! 在這裡依訊息歸類。規格沒有的錯誤用暫定的 E07xx（`docs/note/ai-feedback.md`）。
//! 訊息的措辭一改，這張表就要跟著改：`tests` 會抓到。

/// 執行期死結（暫定）
pub const DEADLOCK: &str = "E0701";

/// (訊息裡的片段，錯誤碼)：第一個符合的勝出
const TABLE: &[(&str, &str)] = &[
    ("does not fit", "E0301"),         // 窄化或 carry 溢位
    ("fixed-point overflow", "E0301"), // 定點數溢位
    ("division by zero", "E0102"),     // 除數為 0
    ("does not own the", "E0702"),     // 通道端點的所有權（暫定）
    ("out of bounds", "E0703"),        // 索引越界（暫定）
    ("deadlock", DEADLOCK),
];

/// 執行期錯誤訊息的錯誤碼；未歸類為 `None`
pub fn code(msg: &str) -> Option<&'static str> {
    TABLE.iter().find(|(k, _)| msg.contains(k)).map(|(_, c)| *c)
}

#[cfg(test)]
mod tests {
    use super::code;

    #[test]
    fn codes() {
        assert_eq!(code("assignment to `acc`: value 9 does not fit i3 (range -4..=3)"), Some("E0301"));
        assert_eq!(code("division by zero"), Some("E0102"));
        assert_eq!(code("task does not own the rx endpoint of channel `c`"), Some("E0702"));
        assert_eq!(code("index 9 out of bounds (length 4)"), Some("E0703"));
        assert_eq!(code("something else"), None);
    }
}
