"""Pytest configuration for the Stage 3 classifier eval suite.

Having conftest.py in this directory ensures pytest adds tests/evals/ to
sys.path so `from metrics import ...` works in the test file.
"""
