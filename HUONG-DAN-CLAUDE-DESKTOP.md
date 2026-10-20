# Duo Switch – Nhiều tài khoản cho Claude Desktop và Codex Desktop

Duo Switch làm hai việc:

- **Claude Desktop:** chuyển qua lại giữa các **hồ sơ**. Mỗi hồ sơ là một thư
  mục app riêng, đăng nhập một tài khoản riêng. Khi chuyển, Duo Switch có thể
  mang theo các **phiên Code Local** của hồ sơ kia, để hai tài khoản cùng thấy
  và tiếp tục được các phiên đó.
- **Codex Desktop** (trên Mac có thể mang tên `ChatGPT.app`): chuyển tài khoản
  đăng nhập. Duo Switch tự thoát Codex rồi mở lại với tài khoản mới, và hiện
  giới hạn 5 giờ / tuần của từng tài khoản (xem mục 5).

## 1. Cần có

- Máy Mac **Apple Silicon** (M1 trở lên), macOS 11 trở lên. Bản này không chạy
  trên Mac Intel.
- **Claude Desktop** và/hoặc **Codex Desktop** đã cài và đã đăng nhập tài
  khoản thứ nhất.
- Không cần cài Rust, Python hay script `claude-switch`.

## 2. Cài đặt

1. Mở file `DuoSwitch-macos-arm64.dmg`, kéo **Duo Switch** vào **Applications**.
2. Lần mở đầu tiên macOS sẽ chặn, vì app chưa được Apple ký. Có hai cách mở:
   - Vào **Cài đặt hệ thống › Quyền riêng tư & Bảo mật**, kéo xuống, bấm
     **Vẫn mở**; hoặc
   - Chạy lệnh sau trong Terminal rồi mở lại app:

     ```bash
     xattr -dr com.apple.quarantine "/Applications/Duo Switch.app"
     ```
3. Duo Switch chạy trên **thanh menu**, không có icon ở Dock. Bấm icon, chọn
   **Mở Duo Switch**.

## 3. Claude Desktop: thiết lập lần đầu (làm một lần)

> Nên để Claude làm xong việc đang chạy trước: mỗi lần đổi hồ sơ, Claude sẽ
> được yêu cầu thoát rồi mở lại. App không bao giờ ép tắt Claude.

1. **Bật chế độ hồ sơ.** Trong tab **Tài khoản**, ở nhóm **Claude Desktop**,
   bấm **Thiết lập…** và giữ tên `tk1`.
   - Duo Switch sao lưu **toàn bộ** thư mục Claude vào
     `~/ClaudeProfiles/_backups/`, rồi biến thư mục đó thành hồ sơ `tk1`.
     Không có gì bị xoá.
   - Khi macOS hỏi **"Duo Switch muốn điều khiển Claude"**, bấm **OK**. Quyền
     này chỉ dùng để yêu cầu Claude thoát.
2. **Tạo một phiên Code Local cho tài khoản 1.** Trong Claude, mở tab
   **Code**, chạy một phiên Local ngắn, rồi kết thúc.
3. **Gắn tài khoản cho `tk1`.** Chuột phải vào `tk1`, chọn **Gắn tài khoản…**,
   chọn đúng tài khoản rồi **Lưu**.
   - Danh sách hiện mã thư mục và số phiên; hãy chọn thư mục có nhiều phiên
     nhất hoặc mới nhất. Sau đó dùng **Đặt tên hiển thị…** để ghi email cho dễ
     nhận.
4. **Thêm tài khoản thứ hai.** Ở mục **Claude Desktop: hồ sơ và phiên chat**,
   bấm **Thêm hồ sơ**, đặt tên `tk2`. Claude sẽ mở trên một hồ sơ trống: đăng
   nhập **tài khoản 2** ở đó.
5. Lặp lại bước 2 và 3 cho `tk2`: tạo một phiên Code Local, rồi gắn tài khoản.
   - Nếu app báo "Tài khoản này đã gắn cho hồ sơ khác", nghĩa là bạn đã đăng
     nhập nhầm tài khoản 1 vào `tk2`.
6. **Thử đồng bộ bằng tay một lần.** Đang ở `tk2`, bấm **Đồng bộ ngay…**.
   Hộp thoại cho xem trước sẽ thêm bao nhiêu phiên; bấm **Thêm** để chạy. Mở
   vài phiên trong Claude để kiểm tra.
7. **Bật auto-sync.** Bật **Auto-sync vào tk1** và **Auto-sync vào tk2**. Từ
   đó, mỗi lần chuyển sang một hồ sơ, các phiên mới của hồ sơ kia sẽ được mang
   theo.

## 4. Claude Desktop: dùng hằng ngày

- **Chuyển tài khoản:** bấm **Dùng** ở hồ sơ muốn chuyển sang, hoặc chọn hồ sơ
  trong menu tray, mục **Claude Desktop**.
- Mỗi lần chuyển, Duo Switch làm lần lượt: yêu cầu Claude thoát, sao lưu hồ sơ
  đang dùng (không gồm cache), chuyển sang hồ sơ mới, chạy auto-sync nếu đang
  bật, rồi mở lại Claude.
- **Hiện email cho hồ sơ:** chuột phải vào hồ sơ, chọn **Đặt tên hiển thị…**
  và ghi email. Email được che mờ, bấm vào mới hiện.

## 5. Codex Desktop

1. Đăng nhập tài khoản 1 trong Codex như bình thường. Trong Duo Switch, khi
   thấy gợi ý **"Đặt tên cho tài khoản này"**, bấm **Đặt tên…** (ví dụ `work`).
2. Bấm **Thêm tài khoản** để đăng nhập tài khoản 2 trên trình duyệt. Tài khoản
   đang dùng không bị đăng xuất.
3. Bấm **Dùng** để chuyển. Nếu Codex đang mở, app hỏi trước, yêu cầu Codex
   thoát, đổi tài khoản rồi mở lại Codex. Lần đầu macOS sẽ hỏi quyền điều
   khiển Codex (hoặc ChatGPT): bấm **OK**.
4. Mỗi tài khoản hiện thanh giới hạn 5 giờ / tuần. Khi tài khoản đang dùng hết
   hạn mức, app báo và gợi ý tài khoản còn trống.

> Đừng dùng `codex login` hay đăng xuất trong Codex: làm vậy sẽ thay hoặc huỷ
> login mà Duo Switch đã lưu.

## 6. Được đồng bộ và không được đồng bộ (Claude Desktop)

| Được mang sang | Không được mang sang |
|---|---|
| Phiên **Code Local** (thẻ phiên trong `claude-code-sessions`) | Chat thường và phiên Cloud: chúng nằm trên máy chủ, theo từng tài khoản |
| Nội dung phiên dùng chung ở `~/.claude/projects`, không bị sao chép hay sửa | Cookie, token, đăng nhập, cấu hình MCP của hồ sơ kia |

Một số phiên sẽ bị **bỏ qua** và hộp thoại sẽ ghi rõ lý do: phiên đã có sẵn,
phiên thiếu nội dung trên máy này, hoặc thẻ phiên có trường lạ mà app không
chắc sửa an toàn. Hồ sơ nguồn không bao giờ bị thay đổi.

## 7. Dữ liệu nằm ở đâu

| Đường dẫn | Nội dung |
|---|---|
| `~/ClaudeProfiles/tk1`, `tk2`, … | Các hồ sơ Claude Desktop |
| `~/Library/Application Support/Claude` | Một **liên kết** (symlink) trỏ vào hồ sơ đang dùng |
| `~/ClaudeProfiles/_meta/` | Tài khoản đã gắn, cài đặt auto-sync, tên hiển thị |
| `~/ClaudeProfiles/_backups/` | Bản sao lưu trước mỗi lần chuyển và mỗi lần đồng bộ. **App không tự xoá**; thỉnh thoảng hãy dọn bớt bản cũ để giải phóng ổ đĩa |
| `~/.pitstop/vault/` | Login Codex đã lưu, mã hoá; khoá nằm trong Keychain |

Mọi thứ chỉ nằm trên máy. Duo Switch không đọc cookie hay token của Claude
Desktop và không gửi các thư mục này đi đâu. Thứ duy nhất gửi ra ngoài là yêu
cầu đọc giới hạn của từng tài khoản Codex, gửi tới chính OpenAI.

## 8. Gặp lỗi

| Thông báo | Cách xử lý |
|---|---|
| "macOS chưa cho Duo Switch yêu cầu Claude thoát" | Vào **Cài đặt hệ thống › Quyền riêng tư & Bảo mật › Tự động hoá**, bật **Claude** dưới **Duo Switch**. Sau mỗi lần cài bản Duo Switch mới có thể phải bật lại. |
| "Claude chưa thoát" | Claude đang bận. Để nó làm xong, bấm Cmd+Q, rồi thử lại. Lúc này chưa có gì bị thay đổi. |
| "Đang có một lần đổi hồ sơ khác chạy" | Chờ thao tác trước xong. Nếu bạn cũng dùng `claude-switch`, đừng chạy hai tool cùng lúc. |
| "Hồ sơ này chưa có dữ liệu Code" | Mở đúng tài khoản trong Claude, tạo một phiên Code Local, kết thúc, rồi gắn tài khoản lại. |
| Hồ sơ không hiện email | Dùng **Đặt tên hiển thị…**. |
| "macOS chưa cho Duo Switch yêu cầu Codex thoát" | Bật **Codex** (hoặc **ChatGPT**) dưới **Duo Switch** trong **Tự động hoá**. |

**Muốn bỏ chế độ hồ sơ**, giữ lại hồ sơ `tk1`: thoát Claude rồi chạy hai lệnh
sau. Lệnh đầu chỉ xoá liên kết, không xoá dữ liệu.

```bash
rm "$HOME/Library/Application Support/Claude"
```
```bash
mv "$HOME/ClaudeProfiles/tk1" "$HOME/Library/Application Support/Claude"
```

## 9. Lưu ý

- Cách Claude Desktop lưu thẻ phiên không có tài liệu chính thức. Sau một bản
  cập nhật lớn của Claude, hãy thử **Đồng bộ ngay…** và kiểm tra lại trước khi
  tin vào auto-sync.
- Duo Switch dùng chung thư mục và file khoá với script `claude-switch`, nên ai
  đang dùng script đó có thể chuyển sang Duo Switch mà không phải thiết lập lại.
