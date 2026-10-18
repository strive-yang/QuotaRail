# QuotaRail Logo

`quotarail.svg` 为 QuotaRail 的可编辑像素终端图标源文件，保留分组、矢量和配色。项目图标按仓库根目录的 MIT 许可证提供，不代表 OpenAI 官方品牌。

`quotarail.png`为512像素预览；`quotarail.ico`包含16、20、24、32、40、48、64像素的32位图像，用作Windows系统托盘图标。Logo保持静态；真实额度与异常信息继续通过常驻条、tooltip及详情呈现，图标中的装饰进度不代表真实额度。

普通Rust构建直接嵌入已提交的ICO，无需Node或SVG运行时。仅重新生成美术资源时，使用安装了`sharp`的Node环境执行：

```text
node scripts/Build-Icon.cjs
```
