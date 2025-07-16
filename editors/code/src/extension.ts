import * as fs from "fs"
import * as path from "path"
import { spawn } from "child_process"

import {
  commands,
  languages,
  window,
  workspace,
  ExtensionContext,
  MarkdownString,
  StatusBarAlignment,
  StatusBarItem,
  TextEditor,
  ThemeColor,
  ViewColumn,
  Uri,
} from 'vscode'
import * as vscode from 'vscode'

import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from 'vscode-languageclient/node'

const logger = window.createOutputChannel("Roughly Extension", { log: true })
const outputChannel = window.createOutputChannel("Roughly")

const EXTENSION_ROOT_DIR = path.dirname(__dirname)
const ROUGHLY_BINARY_NAME = process.platform === "win32" ? "roughly.exe" : "roughly"
const BUNDLED_ROUGHLY_EXECUTABLE = path.join(EXTENSION_ROOT_DIR, "bin", ROUGHLY_BINARY_NAME)

let client: LanguageClient | null = null
let statusBar: StatusBarItem
let status = { health: "started" }
let version: string | undefined

export async function activate(context: ExtensionContext): Promise<void> {
  const { subscriptions, extension } = context
  version = extension.packageJSON.version ?? "<unknown>"

  subscriptions.push(
    workspace.onDidChangeConfiguration(async (change) => {
      if (
        change.affectsConfiguration("roughly.path")
        || change.affectsConfiguration("roughly.args")
        || change.affectsConfiguration("roughly.experimentalFeatures")
      ) {
        const choice = await window.showWarningMessage(
          "Configuration change requires restarting the language server",
          "Restart",
        )
        if (choice === "Restart") {
          await restartClient()
          setTimeout(() => {
            client?.outputChannel.show()
          }, 1500)
        }
      }
    }),
    commands.registerCommand(
      "roughly.restartServer",
      async () => {
        await restartClient()
      }
    ),
    commands.registerCommand(
      "roughly.startServer",
      async () => {
        await restartClient()
      }
    ),
    commands.registerCommand(
      "roughly.stopServer",
      async () => {
        await stopClient()
      }
    ),
    commands.registerCommand(
      "roughly.openLogs",
      async () => {
        if (client?.outputChannel) {
          client.outputChannel.show()
        }
      }
    ),
    commands.registerCommand(
      "roughly.showSyntaxTree",
      async () => {
        await showSyntaxTree()
      }
    )
  )

  statusBar = window.createStatusBarItem(StatusBarAlignment.Left)
  updateStatusBarItem()

  updateStatusBarVisibility(window.activeTextEditor)
  window.onDidChangeActiveTextEditor((editor) => updateStatusBarVisibility(editor))

  registerAstContentProvider(context)

  restartClient()
}

export async function deactivate(): Promise<void> {
  await stopClient()
}

//
// CONTENT PROVIDER
//

class AstContentProvider implements vscode.TextDocumentContentProvider {
  provideTextDocumentContent(uri: vscode.Uri): string {
    const content = astContentMap.get(uri.toString())
    return content || "Failed to load AST content"
  }
}

// Register the content provider
function registerAstContentProvider(context: ExtensionContext) {
  const provider = new AstContentProvider()
  const registration = workspace.registerTextDocumentContentProvider('roughly-ast', provider)
  context.subscriptions.push(registration)
}

//
// SERVER
//

async function restartClient(): Promise<void> {
  const newClient = createClient()
  try {
    await newClient.start()
    void stopClient()
    client = newClient
    setServerStatus({ health: "started" })
  } catch (_) {
    void window.showWarningMessage("Failed to start Roughly language server.")
    setServerStatus({ health: "stopped" })
  }
}

function createClient(): LanguageClient {
  const config = workspace.getConfiguration("roughly")

  const command = process.env.SERVER_PATH
    ?? config.get<string | null>("path")
    ?? (
      fs.existsSync(BUNDLED_ROUGHLY_EXECUTABLE)
        ? BUNDLED_ROUGHLY_EXECUTABLE
        : "roughly"
    )

  const commandArgs = config.get<string[] | null>("args") ?? ["server"]
  const experimentalFeatures = config.get<string[] | null>("experimentalFeatures")
  const experimentalArgs = experimentalFeatures
    ? ["--experimental-features", experimentalFeatures.join(" ")]
    : []
  const args = [...experimentalArgs, ...commandArgs]

  logger.info("using server command:", [command, ...args].join(" "))

  const serverOptions: ServerOptions = {
    command,
    args,
    transport: TransportKind.stdio,
    options: {
      env: {
        ...process.env,
        RUST_LOG: "debug",
      },
    },
  }

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "r" }],
    outputChannel,
  }

  const client = new LanguageClient(
    'roughly',
    'Roughly',
    serverOptions,
    clientOptions
  )

  return client
}


async function stopClient(): Promise<void> {
  if (!client) {
    return
  }

  logger.info("stopping server ...")

  const oldClient = client
  client = null

  // The `stop` call will send the "shutdown" notification to the LSP
  await oldClient.stop()
  // The `dipose` call will send the "exit" request to the LSP which actually tells the child process to exit
  await oldClient.dispose()
}

//
// SYNTAX TREE
//

async function showSyntaxTree(): Promise<void> {
  const editor = window.activeTextEditor
  if (!editor) {
    window.showErrorMessage("No active editor found")
    return
  }

  const document = editor.document
  if (document.languageId !== 'r') {
    window.showErrorMessage("Current file is not an R file")
    return
  }

  const filePath = document.uri.fsPath
  if (!filePath) {
    window.showErrorMessage("Please save the file first")
    return
  }

  const config = workspace.getConfiguration("roughly")
  const roughlyPath = process.env.SERVER_PATH
    ?? config.get<string | null>("path")
    ?? (
      fs.existsSync(BUNDLED_ROUGHLY_EXECUTABLE)
        ? BUNDLED_ROUGHLY_EXECUTABLE
        : "roughly"
    )

  const experimentalFeatures = config.get<string[] | null>("experimentalFeatures")
  const experimentalArgs = experimentalFeatures
    ? ["--experimental-features", experimentalFeatures.join(" ")]
    : []

  try {
    const astOutput = await executeRoughlyDebugAst(roughlyPath, filePath, experimentalArgs)
    const uri = Uri.parse(`roughly-ast:${path.basename(filePath)}.ast`)
    
    // Store the AST output so the content provider can access it
    astContentMap.set(uri.toString(), astOutput)
    
    // Open the virtual document
    const doc = await workspace.openTextDocument(uri)
    await window.showTextDocument(doc, ViewColumn.Beside)
  } catch (error) {
    window.showErrorMessage(`Failed to generate syntax tree: ${error instanceof Error ? error.message : String(error)}`)
  }
}

function executeRoughlyDebugAst(roughlyPath: string, filePath: string, experimentalArgs: string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    const args = [...experimentalArgs, "debug", "ast", filePath]
    const child = spawn(roughlyPath, args, { stdio: 'pipe' })
    
    let stdout = ""
    let stderr = ""
    
    child.stdout.on('data', (data) => {
      stdout += data.toString()
    })
    
    child.stderr.on('data', (data) => {
      stderr += data.toString()
    })
    
    child.on('close', (code) => {
      if (code === 0) {
        resolve(stdout)
      } else {
        reject(new Error(`roughly debug ast failed with code ${code}: ${stderr}`))
      }
    })
    
    child.on('error', (error) => {
      reject(error)
    })
  })
}

// Map to store AST content for virtual documents
const astContentMap = new Map<string, string>()

//
// STATUS BAR
//

type ServerStatus = {
  health: "started" | "stopped"
}

function setServerStatus(newStatus: ServerStatus) {
  status = newStatus
  updateStatusBarItem()
}

function updateStatusBarVisibility(editor: TextEditor | undefined) {

  if (editor != null && languages.match({ language: 'r' }, editor.document) > 0) {
    statusBar.show()
  } else {
    statusBar.hide()
  }
}

function updateStatusBarItem() {
  const icon = ""
  statusBar.tooltip = new MarkdownString("", true)
  statusBar.tooltip.isTrusted = true
  switch (status.health) {
    case "started":
      statusBar.color = undefined
      statusBar.backgroundColor = undefined
      statusBar.command = "roughly.openLogs"
      break
    case "stopped":
      statusBar.tooltip.appendText("Server is stopped")
      statusBar.tooltip.appendMarkdown("\n\n[Start server](command:roughly.startServer)")
      statusBar.color = new ThemeColor("statusBarItem.warningForeground")
      statusBar.backgroundColor = new ThemeColor("statusBarItem.warningBackground")
      statusBar.command = "roughly.startServer"
      statusBar.text = "$(stop-circle) roughly"
      return
  }
  if (statusBar.tooltip.value) {
    statusBar.tooltip.appendMarkdown("\n\n---\n\n")
  }

  const serverVersion = "unknown"
  statusBar.tooltip.appendMarkdown([
    `[Extension Info](command:roughly.serverVersion "Show version and server binary info"): Version ${version}, Server Version ${serverVersion}\n\n---`,
    '[$(terminal) Open Logs](command:roughly.openLogs "Open the server logs")',
    '[$(debug-restart) Restart server](command:roughly.restartServer "Restart the server")',
    '[$(stop-circle) Stop server](command:roughly.stopServer "Stop the server")',
  ].join("\n\n"))

  // if (true) icon = "$(loading~spin) "
  statusBar.text = `${icon}Roughly`
}
