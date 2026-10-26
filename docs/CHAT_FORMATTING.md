# Assistant message formatting

Assistant responses render GitHub-flavored Markdown: headings, bold, italic,
strikethrough, ordered and unordered lists, task lists, quotes, links, inline and
fenced code, and tables. Text is selectable. Wide code and tables scroll
horizontally inside the conversation. Code blocks show their language and a copy
button. Unknown languages and unfinished Markdown remain readable.

User, system, and tool messages remain literal. Message and conversation copy
actions preserve the original Markdown; code copy preserves the parsed code
content (including the Markdown parser's terminating newline). Stored history is
unchanged and receives formatting when reopened.

Links show their destination on hover. Only absolute HTTPS destinations without
credentials open through the application's existing native handler. Unsupported
destinations and failures show feedback in the message. Raw HTML is inert text;
HTML, JavaScript, shell commands, React, and diagram languages are never executed.

## Static mockups and chart images

```markdown
![Checkout mockup](https://example.com/checkout.png)
```

Each image reserves a preview area with alt text. Click **Load image preview** to
fetch it; simply reading or reopening a message never fetches images. The button
shows the destination on hover. Loading contacts that server directly. Only HTTPS
PNG, JPEG, and static WebP are supported, with a 4 MiB download limit, a 4096-pixel
limit per dimension, an 8-megapixel decoded limit, and a 20-second request limit.
Redirects, animation, SVG, local files, assets, data URLs, and other resources are
unsupported. Failure leaves the reserved area in place with a retry button.
Previews are not persisted. No image generation is performed.

## Native charts

Use a closed `ditch-chart` fence with JSON:

````markdown
```ditch-chart
{
  "title": "Weekly signups",
  "type": "bar",
  "labels": ["Mon", "Tue", "Wed"],
  "series": [
    {"name": "This week", "values": [12, 18, 15]},
    {"name": "Last week", "values": [9, 14, 13]}
  ]
}
```
````

`type` is `bar` or `line`. Charts include category labels, series names, and all
numeric values as selectable, accessible text. Negative values and zero are
supported. There must be 1–48 labels and 1–4 series, with exactly one finite
numeric value per label in each series. Values must have absolute magnitude at
most 10^12. Titles, category labels, and series names are required, nonblank
strings of at most 80 characters.

## Color palettes

````markdown
```ditch-palette
{
  "title": "Checkout colors",
  "colors": [
    {"label": "Primary", "hex": "#3155A6"},
    {"label": "Success", "hex": "#18794E"}
  ]
}
```
````

Palettes allow 1–16 colors in six-digit `#RRGGBB` notation. Labels and the title
are required, nonblank strings of at most 80 characters. Each swatch includes its
label and hexadecimal value in visible text and its accessibility label.

Both formats are limited to 16,384 source characters per block. Missing closing
fences, incomplete JSON, invalid schemas, unsupported types, and exceeded limits
fall back to copyable code. Visual blocks also retain a **Copy code** action.
These formats only present supplied data; agent prompts are not modified.
