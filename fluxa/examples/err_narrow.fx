// 窄化必須顯式：u9 → u8 直接賦值，值超出範圍時被攔下
test implicit_narrowing {
    let a: u8 = 200;
    let b: u8 = 100;
    let c = a + b;          // 300，推導寬度 u9，沒問題
    let d: u8 = c;          // 落點型別 u8 → 範圍檢查失敗
}
