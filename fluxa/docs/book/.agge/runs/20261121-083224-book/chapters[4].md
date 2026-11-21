<!-- agent: writer · model: minimax:MiniMax-M3.1-Flash -->
# Prompt

You are writing one chapter of a book.

Book request:
Fluxa 程式設計：用 Kahn 通道寫硬體——從行為模擬、位寬推導、靜態檢查到 FPGA 合成，以及 AI 時代的硬體開發

The book should have about 12 chapters.
Write the whole book in 繁體中文.
Target audience: 會寫程式、想學硬體設計的學生與工程師.
Style / format requirements: @/private/tmp/claude-502/-Users-Shared-ccc-project-rueda/8466f906-f1df-4a29-ad8b-4e9895fb6b43/scratchpad/bookstyle.md

Full outline (read `outline.md` for details):
已完成書籍規劃：

- `outline.md`：包含全書定位、12 章詳細大綱、章節銜接、案例路線、程式碼與視覺素材清單，以及編輯品質檢核表。
- `README.md`：包含完整書名、副標題、序言、學習成果、閱讀路線、版本說明，以及連至全部 12 章的目錄。

```json
[
  {
    "file": "chapters/01-from-software-to-hardware.md",
    "title": "第 1 章　從順序程式到空間資料流：Fluxa 的硬體思維",
    "brief": "建立硬體設計的共同語言，說明吞吐、延遲、面積與時脈週期，並以 FIR 示範如何先定義 token 契約。"
  },
  {
    "file": "chapters/02-language-tour.md",
    "title": "第 2 章　Fluxa 語言地圖：從第一個 task 到可測試元件",
    "brief": "介紹 Fluxa 的頂層項目、函式與 task、控制流、泛型及測試，並完成第一個可測試硬體元件。"
  },
  {
    "file": "chapters/03-types-and-widths.md",
    "title": "第 3 章　型別就是位寬：值域推導、窄化與定點數",
    "brief": "從封閉值域推導整數位寬，處理落點檢查、窄化、明確回繞與飽和，並完整解說定點數及捨入策略。"
  },
  {
    "file": "chapters/04-kahn-channels.md",
    "title": "第 4 章　Kahn 通道：把演算法組成程序網路",
    "brief": "解釋 task、token、有界 FIFO、線性端點所有權、broadcast、背壓與死結，將 FIR 組裝成程序網路。"
  },
  {
    "file": "chapters/05-behavioral-simulation.md",
    "title": "第 5 章　行為模擬：排程無關的執行模型與可重現測試",
    "brief": "說明兩種排程器與 Kahn 確定性，建立以 assert、trace、stats 為核心的行為 oracle 與除錯流程。"
  },
  {
    "file": "chapters/06-static-checking.md",
    "title": "第 6 章　讓錯誤提前發生：靜態檢查、值域分析與診斷",
    "brief": "介紹靜態值域、累加器、通道守恆、端點所有權及資源檢查，並示範如何解讀 E0102、E0301、E0412 等診斷。"
  },
  {
    "file": "chapters/07-task-to-hardware.md",
    "title": "第 7 章　從 task 到硬體：FSM、資料路徑與控制流降階",
    "brief": "拆解 Fluxa 從解析、展開、位寬推導到 CFG、FSM 與網表生成的流程，將控制流映射成狀態與資料路徑。"
  },
  {
    "file": "chapters/08-timing-and-resources.md",
    "title": "第 8 章　時序與資源：管線、累加器、暫存器與記憶體",
    "brief": "區分 initiation interval、latency 與吞吐量，運用 pipeline、unroll、serial、@reg、carry 與 mem 表達硬體排程。"
  },
  {
    "file": "chapters/09-synthesis-to-chip.md",
    "title": "第 9 章　從網表到晶片：頂層介面、FPGA 與 ASIC 流程",
    "brief": "說明 top-level 選擇、參數化、valid/ready/close 串流介面，以及 FPGA 位元流、ASIC GDS 與資源時序報告。"
  },
  {
    "file": "chapters/10-cross-layer-verification.md",
    "title": "第 10 章　同一份設計的三次驗證：sim、emu 與 gatesim 等價",
    "brief": "比較行為直譯、編譯後模擬與閘級模擬，建立以 token 序列、断言和關閉行為為判準的差異測試矩陣。"
  },
  {
    "file": "chapters/11-dsp-case-studies.md",
    "title": "第 11 章　完整案例：FIR、影像視窗與脈動矩陣乘法",
    "brief": "綜合型別、網路、檢查與合成知識，完成 FIR、3×3 影像視窗和脈動矩陣乘法元件及完整驗證流程。"
  },
  {
    "file": "chapters/12-ai-era-hardware-development.md",
    "title": "第 12 章　AI 時代的硬體開發：從規格到晶片的代理式工作流",
    "brief": "建立由規格驅動的 AI agent 工作流，涵蓋提示模板、風險關卡、證據閘門、供應鏈安全與團隊治理。"
  }
]
```

Your chapter: 第 4 章　Kahn 通道：把演算法組成程序網路
File: chapters/04-kahn-channels.md
Brief: 解釋 task、token、有界 FIFO、線性端點所有權、broadcast、背壓與死結，將 FIR 組裝成程序網路。

Write the COMPLETE chapter in Markdown to `chapters/04-kahn-channels.md` with write_file (use append="true" to continue a long chapter in several parts). Start with a `# ` heading. Reply with a 3-line summary.

# Output

<edit_file path="chapters/04-kahn-channels.md"><old>7. 將 FIR 組成「來源 → FIR → 消费者」網路，並為每條通道寫出元素型別、順序、深度與關閉責任。]<]minimax[>[old><new>7. 將 FIR 組成「來源 → FIR → 消費者」網路，並為每條通道寫出元素型別、順序、深度與關閉責任。
