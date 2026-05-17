# OPC Role Prompts — Attribution

The `opc-*.md` prompts in this directory are derived from the
[VoltAgent/awesome-claude-code-subagents](https://github.com/VoltAgent/awesome-claude-code-subagents)
collection (MIT License, copyright the VoltAgent contributors).

## Mapping

| OPC role file              | Source                                                                  |
|----------------------------|-------------------------------------------------------------------------|
| `opc-product.md`           | `categories/08-business-product/product-manager.md`                     |
| `opc-engineering.md`       | `categories/01-core-development/fullstack-developer.md`                 |
| `opc-marketing.md`         | `categories/08-business-product/content-marketer.md`                    |
| `opc-sales.md`             | `categories/08-business-product/sales-engineer.md`                      |
| `opc-ops.md`               | `categories/08-business-product/project-manager.md`                     |
| `opc-legal.md`             | `categories/08-business-product/legal-advisor.md`                       |
| `opc-finance.md`           | merge of `07-specialized-domains/{quant-analyst,risk-manager}.md`       |

The YAML frontmatter (`name`/`description`/`tools`/`model`) was stripped so
only the body prompt remains. A small OPC-context preamble is prepended at
runtime by `build_agent_system_prompt()` in `tools/src/lib.rs`.

## License notice (preserved from upstream)

> MIT License — Copyright (c) the VoltAgent contributors

See <https://github.com/VoltAgent/awesome-claude-code-subagents/blob/main/LICENSE>
for the full text.
