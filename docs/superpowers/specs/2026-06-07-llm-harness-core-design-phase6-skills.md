### 5.5 Skills 与 PromptTemplates

#### Skills

```rust
pub struct Skill {
    pub name:                     String,   // 校验：小写字母+数字+连字符，≤64
    pub label:                    Option<String>,
    pub description:              String,   // 校验：非空，≤1024
    pub content:                  String,
    pub source:                   PathBuf,
    pub disable_model_invocation: bool,     // true: 不进 system prompt，仅供显式调用
}

pub struct SkillDiagnostic { pub source: PathBuf, pub level: DiagnosticLevel, pub message: String }
// DiagnosticLevel 定义见 §3.1

pub struct SourcedSkill {
    pub skill:  Skill,
    pub source_tag: String,  // 调用方提供的来源标记（如 "user-config", "project-local"）
}

/// 递归扫描目录；每目录只取第一个 SKILL.md（不递归子目录的 SKILL.md）。
/// 校验 name 匹配父目录名。遵守 .gitignore / .ignore / .fdignore。
/// 解析符号链接。
pub async fn load_skills(
    env:  &dyn ExecutionEnv,
    dirs: &[PathBuf],
) -> (Vec<Skill>, Vec<SkillDiagnostic>);

pub async fn load_sourced_skills(
    env:  &dyn ExecutionEnv,
    dirs: &[(String, PathBuf)],  // (source_tag, dir)
) -> (Vec<SourcedSkill>, Vec<SkillDiagnostic>);

/// 注入 system prompt（仅 disable_model_invocation=false 的 skill）
pub fn format_skills_for_system_prompt(skills: &[Skill]) -> String;

/// 显式调用：将 skill 内容包装为 `<skill name="...">...</skill>` 块，作为 user 消息注入
pub fn format_skill_invocation(skill: &Skill, additional_instructions: Option<&str>) -> String;
```

#### PromptTemplates

**位置参数 + shell-style 引号解析**，对齐 pi-agent-core：占位符 `$1`、`$2`、`$@`、`$ARGUMENTS`、`${@:N}`、`${@:N:L}`。

```rust
pub struct PromptTemplate {
    pub name:    String,
    pub content: String,
    pub source:  PathBuf,
}

pub async fn load_prompt_templates(
    env:  &dyn ExecutionEnv,
    dirs: &[PathBuf],
) -> (Vec<PromptTemplate>, Vec<SkillDiagnostic>);

/// args 为位置参数列表；invoke 内部 shell-style 解析输入
pub fn invoke_template(
    template: &PromptTemplate,
    args:     &[String],
) -> Result<String, TemplateError>;
```

**测试策略：** Skills 和 Templates 的加载逻辑依赖 `dyn ExecutionEnv`。v1 **不**提供 `InMemoryEnv` mock——测试通过 `tempfile::TempDir` + `OsEnv` 真实运行：在临时目录布置 `SKILL.md` / 模板文件，调用 `load_skills()`，验证返回值。这样既覆盖了真实 fs 行为，又避免维护双重 env 实现。后续若需要 hermetic 测试再引入 `InMemoryEnv`。
