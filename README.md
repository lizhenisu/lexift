# Lexift

Lexift 是一款使用 Rust 与 Slint 构建的 Windows 桌面划词翻译工具，支持屏幕几何标注与聚光灯讲解。

## 快速开始

从 [GitHub Releases](https://github.com/lizhenisu/lexift/releases) 下载 Windows `exe` 安装包，或下载 portable ZIP 解压后直接运行 `lexift.exe`。

首次使用：

1. 打开 Settings → Service providers，在 DeepL API Key 字段中保存 API Key。
2. 支持 Windows 应用中用鼠标拖选或双击选中文字，松开后可点击悬浮工具条的翻译或复制按钮。
3. 也可以按 `Alt + X` 获取并翻译选中内容；按 `Alt + A` 打开标注工具条，使用矩形、椭圆和聚光灯辅助讲解。

## 界面预览

<table>
  <tr>
    <td align="center" width="50%"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/01-appearance-light.png" alt="Lexift 浅色外观设置" width="440"><br><strong>浅色外观</strong></td>
    <td align="center" width="50%"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/02-appearance-dark.png" alt="Lexift 深色外观设置" width="440"><br><strong>深色外观</strong></td>
  </tr>
  <tr>
    <td align="center" width="50%"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/15-popup-en-zh-light.png" alt="Lexift 浅色翻译窗口：英文原文及 DeepL 返回的中文译文" width="440"><br><strong>选中文字，理解每一个好想法</strong></td>
    <td align="center" width="50%"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/17-geometry-controls.png" alt="Lexift 几何标注：矩形突出重点，工具条展开形状、线宽、圆角与颜色参数" width="440"><br><strong>框出重点，让讲解更清晰</strong></td>
  </tr>
  <tr>
    <td align="center"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/16-rectangle-presentation.png" alt="Lexift 使用矩形框选演示文稿中的重点内容" width="440"><br><strong>几何标注，框出重点</strong></td>
    <td align="center"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/19-spotlight-presentation.png" alt="Lexift 聚光灯突出显示演示文稿中的第三张卡片" width="440"><br><strong>聚光灯，让讲解更聚焦</strong></td>
  </tr>
</table>

<details>
<summary><strong>查看全部 20 张截图</strong></summary>

点击图片可查看原图。

<table>
  <tr>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/01-appearance-light.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/01-appearance-light.png" alt="Lexift 实拍：浅色外观" width="440"></a><br><strong>01 · 浅色外观</strong></td>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/02-appearance-dark.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/02-appearance-dark.png" alt="Lexift 实拍：深色外观" width="440"></a><br><strong>02 · 深色外观</strong></td>
  </tr>
  <tr>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/03-hotkeys-dark.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/03-hotkeys-dark.png" alt="Lexift 实拍：翻译与标注快捷键" width="440"></a><br><strong>03 · 翻译与标注快捷键</strong></td>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/04-selection-assistant-dark.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/04-selection-assistant-dark.png" alt="Lexift 实拍：划词助手设置" width="440"></a><br><strong>04 · 划词助手设置</strong></td>
  </tr>
  <tr>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/05-deepl-provider-dark.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/05-deepl-provider-dark.png" alt="Lexift 实拍：DeepL 服务配置" width="440"></a><br><strong>05 · DeepL 服务配置</strong></td>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/06-deepl-inspiration-dark.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/06-deepl-inspiration-dark.png" alt="Lexift 实拍：灵感与跨文化交流" width="440"></a><br><strong>06 · 灵感与跨文化交流</strong></td>
  </tr>
  <tr>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/07-popup-zh-en-dark.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/07-popup-zh-en-dark.png" alt="Lexift 实拍：中文 → 英文" width="440"></a><br><strong>07 · 中文 → 英文</strong></td>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/08-popup-en-zh-dark.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/08-popup-en-zh-dark.png" alt="Lexift 实拍：英文 → 简体中文" width="440"></a><br><strong>08 · 英文 → 简体中文</strong></td>
  </tr>
  <tr>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/09-popup-en-ja-dark.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/09-popup-en-ja-dark.png" alt="Lexift 实拍：英文 → 日文" width="440"></a><br><strong>09 · 英文 → 日文</strong></td>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/10-popup-en-de-dark.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/10-popup-en-de-dark.png" alt="Lexift 实拍：英文 → 德文" width="440"></a><br><strong>10 · 英文 → 德文</strong></td>
  </tr>
  <tr>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/11-popup-en-it-dark.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/11-popup-en-it-dark.png" alt="Lexift 实拍：英文 → 意大利文" width="440"></a><br><strong>11 · 英文 → 意大利文</strong></td>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/12-interface-languages.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/12-interface-languages.png" alt="Lexift 实拍：12 种界面语言" width="440"></a><br><strong>12 · 12 种界面语言</strong></td>
  </tr>
  <tr>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/13-appearance-chinese-light.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/13-appearance-chinese-light.png" alt="Lexift 实拍：中文界面 · 浅色外观" width="440"></a><br><strong>13 · 中文界面 · 浅色外观</strong></td>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/14-translation-settings-chinese.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/14-translation-settings-chinese.png" alt="Lexift 实拍：中文界面 · 翻译设置" width="440"></a><br><strong>14 · 中文界面 · 翻译设置</strong></td>
  </tr>
  <tr>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/15-popup-en-zh-light.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/15-popup-en-zh-light.png" alt="Lexift 实拍：让好想法被理解" width="440"></a><br><strong>15 · 让好想法被理解</strong></td>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/16-rectangle-presentation.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/16-rectangle-presentation.png" alt="Lexift 实拍：矩形 · 聚焦阅读" width="440"></a><br><strong>16 · 矩形 · 聚焦阅读</strong></td>
  </tr>
  <tr>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/17-geometry-controls.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/17-geometry-controls.png" alt="Lexift 实拍：几何图形参数" width="440"></a><br><strong>17 · 几何图形参数</strong></td>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/18-ellipse-presentation.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/18-ellipse-presentation.png" alt="Lexift 实拍：椭圆 · 连接观点" width="440"></a><br><strong>18 · 椭圆 · 连接观点</strong></td>
  </tr>
  <tr>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/19-spotlight-presentation.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/19-spotlight-presentation.png" alt="Lexift 实拍：矩形聚光灯 · 突出重点" width="440"></a><br><strong>19 · 矩形聚光灯 · 突出重点</strong></td>
    <td align="center" width="50%"><a href="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/20-spotlight-focus.png"><img src="https://raw.githubusercontent.com/lizhenisu/posters/main/lexift/screenshots/20-spotlight-focus.png" alt="Lexift 实拍：椭圆聚光灯 · 聚焦讲解" width="440"></a><br><strong>20 · 椭圆聚光灯 · 聚焦讲解</strong></td>
  </tr>
</table>

</details>

## 开发

```console
cargo run
```

M1 Mock 演示：

```console
cargo run --features m1-demo
```

## 许可证

[GPL-3.0](LICENSE)
