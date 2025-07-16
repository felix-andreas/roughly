import * as assert from 'assert'
import * as vscode from 'vscode'

suite('Extension Test Suite', () => {
  test('Extension should be present', () => {
    assert.ok(vscode.extensions.getExtension('felix-andreas.roughly'))
  })

  test('Should register show syntax tree command', async () => {
    const commands = await vscode.commands.getCommands(true)
    assert.ok(commands.includes('roughly.showSyntaxTree'))
  })
})