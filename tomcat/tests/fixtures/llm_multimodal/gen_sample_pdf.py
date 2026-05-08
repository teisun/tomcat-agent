#!/usr/bin/env python3
"""Generate a small real PDF and print its base64 to stdout.

Usage:
    pip install reportlab
    python3 gen_sample_pdf.py > sample_pdf_b64.txt

The integration test only `include_str!`s `sample_pdf_b64.txt`; this script is
provided so the fixture can be regenerated on demand without bundling reportlab
into CI.
"""

import base64
import io

from reportlab.pdfgen import canvas

buf = io.BytesIO()
c = canvas.Canvas(buf)
c.drawString(72, 720, "Hello PDF content for LLM summarize test")
c.save()
print(base64.b64encode(buf.getvalue()).decode("ascii"))
