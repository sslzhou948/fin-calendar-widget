# fin-calendar-widget 桌面财经日历挂件

对标金十数据"财经日历壁纸"的自研桌面挂件：以**附着在壁纸上的透明叠加层**形式展示未来一周财经日历，主要服务美股、期货、汇率交易者。

- 技术栈：Tauri v2（Rust + 系统 WebView），一套代码编 Windows / macOS
- 数据源：Investing.com 官方小工具接口（主）+ JSON 接口（备），可插拔、故障自动降级
- 刷新：事件驱动，零轮询 —— 每周日 0 点全量 + 开机对账补拉 + 公布后约 2 分钟唤醒更新实际值
- 显示：全部北京时间（三类行：数据行三栏 / 事件行紧凑单行 / 假期行淡化）；中性着色（超预期绿、低预期红、符合预期灰）；圆形国旗 + 国家缩写；毛玻璃面板（可开关，默认开）；纯点击穿透

详细需求见 [`PRD.md`](./PRD.md)，技术方案见 [`docs/TECH-DESIGN.md`](./docs/TECH-DESIGN.md)。

## 本地开发

```bash
npm install
npx tauri dev      # 调试运行
npx tauri build    # 打安装包（Windows: msi/nsis；macOS: dmg）
```

## 云构建

`.github/workflows/build.yml`（需手动上传，见文件头注释）：push tag `v*` 后在 Windows / macOS 云主机自动构建并发布 draft Release。
