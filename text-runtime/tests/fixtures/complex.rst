======================
Complex RST Fixture
======================

Introduction
============
This is a complex reStructuredText document. It includes **bold** and *italic* text.

.. note::
   This is an admonition directive.

Lists
-----
* Item 1
* Item 2
  * Subitem A
  * Subitem B

Grid Table
----------
+------------+------------+
| Header 1   | Header 2   |
+============+============+
| Row 1, 1   | Row 1, 2   |
+------------+------------+
| Row 2, 1   | Row 2, 2   |
+------------+------------+

Code Block
----------
.. code-block:: python

   def hello_world():
       print("Hello, World!")