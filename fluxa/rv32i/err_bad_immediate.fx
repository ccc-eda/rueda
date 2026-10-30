// 位寬型別在組譯期就攔下超出欄位寬度的立即值（i12 只到 2047）
test immediate_too_wide {
    let w = addi(1, 0, 3000);
}