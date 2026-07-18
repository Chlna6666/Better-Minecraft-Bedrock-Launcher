#!/usr/bin/env node

const fs = require('fs');
const path = require('path');

const messageFile = process.argv[2];
if (!messageFile) {
  console.error('用法: node scripts/validate-commit.js <提交信息文件>');
  process.exit(2);
}

const message = fs.readFileSync(path.resolve(messageFile), 'utf8').split(/\r?\n/, 1)[0].trim();
const pattern = /^(feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)(\([^()]+\))?!?: .+/u;

if (!pattern.test(message)) {
  console.error('提交信息格式错误。请使用：类型(范围): 中文描述');
  console.error('类型：feat、fix、docs、style、refactor、perf、test、build、ci、chore、revert');
  process.exit(1);
}
