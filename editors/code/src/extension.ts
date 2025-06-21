import * as fs from "fs"
import * as path from "path"

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
} from 'vscode'

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

export async function activate({ subscriptions, extension, }: ExtensionContext): Promise<void> {
  version = extension.packageJSON.version ?? "<unknown>"

  subscriptions.push(
    workspace.onDidChangeConfiguration(async (change) => {
      if (
        change.affectsConfiguration("roughly.path", undefined)) {
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
        await restartClient
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
    )
  )

  statusBar = window.createStatusBarItem(StatusBarAlignment.Left)
  updateStatusBarItem()

  updateStatusBarVisibility(window.activeTextEditor)
  window.onDidChangeActiveTextEditor((editor) => updateStatusBarVisibility(editor))

  restartClient()
}

export async function deactivate(): Promise<void> {
  await stopClient()
}

//
// SERVER
//

async function restartClient(): Promise<void> {
  try {
    const newClient = createClient()
    newClient.start()
    void stopClient()
    client = newClient
    setServerStatus({ health: "started" })
  } catch (reason) {
    // void window.showWarningMessage("Failed to start Roughly language server.")
    setServerStatus({ health: "stopped" })
  }
}

function createClient(): LanguageClient {
  const config = workspace.getConfiguration("roughly")

  const command = process.env.SERVER_PATH
    ?? config.get<string>("path")
    ?? (
      fs.existsSync(BUNDLED_ROUGHLY_EXECUTABLE)
        ? BUNDLED_ROUGHLY_EXECUTABLE
        : "roughly"
    )

  const args = config.get<string[]>("args", ["server"])

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
  let icon = ""
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
