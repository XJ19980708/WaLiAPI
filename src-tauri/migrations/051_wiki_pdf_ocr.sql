-- Wiki 项目新增 PDF 摄入视觉模型配置（用于扫描页/图文页 OCR）。
-- 与摄入模型平行：channel_id + model 双字段；均 NULL/空 = 不启用 PDF OCR 视觉识别，
-- 此时扫描版 PDF 摄入会返回 OCR_MODEL_NOT_CONFIGURED 提示。
ALTER TABLE wiki_projects ADD COLUMN pdf_ocr_channel_id TEXT;
ALTER TABLE wiki_projects ADD COLUMN pdf_ocr_model TEXT;