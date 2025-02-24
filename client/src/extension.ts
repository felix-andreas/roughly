import {
  commands,
  languages,
  window,
  workspace,
  ExtensionContext,
  MarkdownString,
  StatusBarAlignment,
  TextEditor,
  ThemeColor,
} from 'vscode'

import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from 'vscode-languageclient/node'

let client: LanguageClient
let statusBar
let version
let status = { health: "started" }

export function activate({ subscriptions, extension }: ExtensionContext) {
  const config = workspace.getConfiguration("roughly")
  const command = process.env.SERVER_PATH || config.get<string>("path", "roughly")
  const args = config.get<string[]>("args", ["lsp"])
  version = extension.packageJSON.version ?? "<unknown>"

  console.log("using server command:", [command, ...args].join(" "))
  const outputChannel = window.createOutputChannel("Roughly");

  client = (() => {
    const serverOptions: ServerOptions = {
      command,
      // args, // TODO: uncomment after some versions
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

    return new LanguageClient(
      'roughly',
      'Roughly',
      serverOptions,
      clientOptions
    )
  })()

  subscriptions.push(
    workspace.onDidChangeConfiguration(async (change) => {
      if (
        change.affectsConfiguration("roughly.path", undefined)) {
        const choice = await window.showWarningMessage(
          "Configuration change requires restarting the language server",
          "Restart",
        )
        if (choice === "Restart") {
          await client.restart()
          setTimeout(() => {
            client.outputChannel.show()
          }, 1500)
        }
      }
    }),
    commands.registerCommand(
      "roughly.restartLanguageServer",
      async () => {
        if (client.isRunning) {
          await client.restart()
        } else {
          await client.start()
        }
        setServerStatus({ health: "started" })
      }
    ),
    commands.registerCommand(
      "roughly.startLanguageServer",
      async () => {
        await client.start()
        setServerStatus({ health: "started" })
      }
    ),
    commands.registerCommand(
      "roughly.stopLanguageServer",
      async () => {
        await client.stop()
        setServerStatus({ health: "stopped" })
      }
    ),
    commands.registerCommand(
      "roughly.openLogs",
      async () => {
        if (client.outputChannel) {
          client.outputChannel.show();
        }
      }
    )
  )

  statusBar = window.createStatusBarItem(StatusBarAlignment.Left)
  updateStatusBarItem()

  updateStatusBarVisibility(window.activeTextEditor)
  window.onDidChangeActiveTextEditor((editor) => updateStatusBarVisibility(editor))

  client.start() // this also launches the server
}

type ServerStatus = {
  health: "started" | "stopped"
}

function setServerStatus(newStatus: ServerStatus) {
  status = newStatus;
  updateStatusBarItem();
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
      statusBar.tooltip.appendMarkdown("\n\n[Start server](command:roughly.startLanguageServer)")
      statusBar.color = new ThemeColor("statusBarItem.warningForeground")
      statusBar.backgroundColor = new ThemeColor("statusBarItem.warningBackground")
      statusBar.command = "roughly.startLanguageServer"
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
    '[$(debug-restart) Restart server](command:roughly.restartLanguageServer "Restart the server")',
    '[$(stop-circle) Stop server](command:roughly.stopLanguageServer "Stop the server")',
  ].join("\n\n"))

  // if (true) icon = "$(loading~spin) "
  statusBar.text = `${icon}Roughly`
}


export function deactivate(): Thenable<void> | undefined {
  if (!client) {
    return undefined
  }
  return client.stop()
}
