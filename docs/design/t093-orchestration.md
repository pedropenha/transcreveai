# T-093 — Ditado traduzido por sessão

Implementação autorizada em 2026-10-03. A ação traduz áudio para inglês no motor local e insere o resultado no destino do ditado. É independente do Command Mode T-056 e do assistente T-091. O acionamento por voz e a versão de lançamento continuam sem definição; não condicionam a entrega do atalho explícito.

## 1. Pesquisa e reuso

Reutilizar catálogo (`supports_translation`), `TranscriptionManager`, `LocalSttProvider`, coordenador de sessões, captura de contexto e inserção existentes. Não adicionar dependências. A capacidade é determinada pelo catálogo/metadata, nunca pelo nome do modelo; Turbo não suporta tradução, Medium e Large compatíveis suportam. A preferência global `translate_to_english` e os provedores de ditado/reunião não são alterados.

Pesquisa externa: [documentação primária Whisper](https://github.com/openai/whisper#command-line-usage) confirma a limitação de tradução do Turbo e recomenda Medium/Large para essa tarefa. A busca autenticada do GitHub CLI estava indisponível; foram usados código local e documentação pública. Consulta a novos pacotes não se aplica porque o motor e o catálogo já existem no projeto.

## 2. Arquitetura e contrato

- Binding separado `transcribe_translate`, sujeito à mesma captura de atalhos, validação de conflitos e ativação configurável do ditado.
- Setting opcional `translation_model_id`: escolha específica da tradução. Sem escolha, usar o modelo do ditado somente se compatível.
- Comando `change_translation_model_setting`: validar compatibilidade local ao configurar. O modelo pode precisar de download; a gravação só começa quando estiver disponível.
- A execução usa um snapshot privado das opções, com tradução habilitada somente nessa sessão, modelo local compatível e sem fallback para nuvem.
- O idioma de origem nesse snapshot é automático, inclusive quando o idioma global do ditado é inglês; a preferência armazenada não muda.
- A próxima execução comum respeita seu modelo e suas opções originais. Falha ou cancelamento não habilita tradução globalmente.
- Contexto alvo e clipboard seguem o caminho existente de inserção; a configuração ocorre no Hub e a sessão começa pelo atalho no aplicativo de destino.
- Como o motor é compartilhado com o transcritor de reuniões, uma gravação de reunião ativa bloqueia o início do ditado traduzido com aviso explícito. Uma carga de modelo já em andamento também exige nova tentativa após terminar, sem abrir o microfone ou substituir o modelo em uso silenciosamente.
- A entrega traduzida revalida cancelamento e destino nos pontos de injeção, incluindo após o atraso de colagem. Scripts externos não oferecem esse controle e são recusados nesse modo; o ditado comum conserva seu dispatcher anterior.

## 3. Interação

Configurações → Transcrição → Idiomas apresenta tradução por ditado, seletor de modelo e instruções para configurar/usar o atalho. Modelo incompatível, ausente ou não baixado é explicitamente indicado; a tela de Modelos oferece download existente. Atalhos inclui a nova ação. Strings em pt-BR/en; navegação por teclado e nomes acessíveis seguem os componentes existentes.

## 4. Aceitação e execução

1. Sessão traduzida usa modelo compatível e tradução local; a sessão comum seguinte mantém suas preferências, inclusive quando a tradução falha ou é cancelada.
2. Modelo incompatível/não disponível impede a captura antes de qualquer inserção ou mutação do clipboard; a escolha não modifica modelo/provedor do ditado.
3. Cancelamento impede inserção. A inserção bem-sucedida mantém o destino e respeita a política existente de restauração do clipboard.

Backend e frontend executam planejamento → TDD → implementação com revisão independente ao integrar. Testes unitários exercitam isolamento, capability gating e sessões; testes Playwright exercitam escolha, atalhos, erros e acessibilidade com IPC simulado. Build/lint/formatação e cobertura acompanham a mudança. Smoke de áudio, atalho, foco e colagem com o aplicativo nativo é uma etapa distinta dos testes de navegador.

Resultados finais e limitações serão registrados em `t093-validation.md`; a tarefa permanece aberta enquanto houver portões necessários sem evidência.
