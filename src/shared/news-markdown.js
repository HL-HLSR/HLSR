"use strict";

const markdown = require("./guide-markdown");
const options = Object.freeze({ headings: true, preserveCodeIndent: true });
function render(body) { return markdown.render(body, options); }
module.exports = { render };
