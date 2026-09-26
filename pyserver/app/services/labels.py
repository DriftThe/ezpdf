"""PP-DocLayoutV3 label → Mistral `blocks[].type`. The pipeline's own labels stay the source of truth
(the client reads them back from the block's extra `label` field), but a Mistral-shaped response must
declare one of Mistral's block types so the official clients can parse it.
"""

from __future__ import annotations

# Mistral's block vocabulary (mistralai client: ocrtextblock/ocrtitleblock/… — 13 Literal types).
MISTRAL_BLOCK_TYPES = frozenset({
    "text", "title", "list", "table", "image", "equation", "caption", "code",
    "references", "aside_text", "header", "footer", "signature",
})

# PP-DocLayoutV3's 21 classes → the closest Mistral type. Anything unlisted falls back to "text",
# which is what Mistral calls a plain body block.
LABEL_TO_MISTRAL: dict[str, str] = {
    "text": "text",
    "content": "text",
    "abstract": "text",
    "footnote": "text",
    "vision_footnote": "text",
    "number": "text",
    "reference_content": "text",
    "doc_title": "title",
    "paragraph_title": "title",
    "figure_title": "caption",
    "table": "table",
    "formula": "equation",
    "formula_number": "equation",
    "image": "image",
    "chart": "image",
    "seal": "signature",
    "algorithm": "code",
    "reference": "references",
    "aside_text": "aside_text",
    "header": "header",
    "footer": "footer",
}


def mistral_block_type(label: str) -> str:
    """Mistral type for one of our labels (unknown → "text"; never returns something off the list)."""
    kind = LABEL_TO_MISTRAL.get(label, "text")
    return kind if kind in MISTRAL_BLOCK_TYPES else "text"
