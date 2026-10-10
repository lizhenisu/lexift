# 标注字体目录与默认选择

文字、水印和序号共用本机已安装字体目录。目录按显示名称忽略大小写排序，同名时按原名称稳定排序并去重；名称保持原样，不翻译或按拼音排序。

## 默认字体

首次使用标注字体时初始化目录，无需先打开菜单。依次采用第一个已安装的候选；候选缺失时解析系统无衬线字体。

候选匹配后，使用字体自带的语言名称元数据，优先选择当前界面语言对应、且已存在于目录中的名称。例如简体中文默认选择“微软雅黑”，而不是“Microsoft YaHei”。没有对应名称时保留候选原名；系统无衬线回退同样处理。中英文名称仍全部保留，手动选择英文名称不会被转换。名称对应关系只在目录初始化时建立。

| 界面语言 | 候选顺序 |
| --- | --- |
| 简体中文 | Microsoft YaHei、Noto Sans CJK SC |
| 繁体中文 | Microsoft JhengHei、Noto Sans CJK TC |
| 日语 | Yu Gothic、Meiryo、Noto Sans CJK JP |
| 韩语 | Malgun Gothic、Noto Sans CJK KR |
| 英语及其他欧洲语言 | Segoe UI、Noto Sans、Arial |

三个工具分别保存本进程内的手动字体偏好。语言切换只更新未手动选择的默认字体；已有对象、当前编辑草稿保持实际字体。对象参数回填不记为手动选择。关闭标注会话保留偏好，退出进程后不保留。

## 菜单定位

字体菜单展开及窗口 DPI 布局完成后，将当前字体滚动至列表中部；靠近首尾时限制在滚动范围内。上下方向键、Home 和 End 更新选中行，并确保该行可见。推荐字体不改变目录排序。

## 验证

- UI 测试覆盖 12 种界面语言、候选与回退、手动偏好、对象回填、名称排序与去重。
- 软件窗口测试覆盖 100%／125% DPI 的长列表初始定位及键盘导航。
- 125% DPI 实际截图保存于 `target/font-menu-text-125.png`、`target/font-menu-watermark-125.png`、`target/font-menu-sequence-125.png`。当前环境只有单屏，双屏及负坐标实机交互未验证。
- 本地名称测试覆盖全部 12 种语言、名称不在目录中的回退、系统字体回退及多字重记录去重；中文实际截图保存于 `target/font-localized-text-menu-125.png`、`target/font-localized-watermark-menu-125.png`、`target/font-localized-sequence-menu-125.png`。
